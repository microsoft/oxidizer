// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Wake, Waker};
use std::time::Duration;

use mio::{Events, Poll, Token};
use observed::Sink;
use performables::arc::Arc;

use super::primary::PrimaryDriver;
use super::secondary::{SecondaryDriver, SecondaryShared, drain_secondary_events};
use super::{COMPLETION_BUDGET, EVENT_CAPACITY};
use crate::worker::WorkerState;

#[test]
fn secondary_background_failure_becomes_driver_error() {
    let (mut driver, poll_waker, registry) = SecondaryDriver::new().unwrap();
    let worker = Arc::new(WorkerState::new(
        std::thread::current().id(),
        registry,
        Waker::noop().clone(),
        Sink::noop(),
    ));
    driver.attach(Arc::clone(&worker));
    driver.shared.fail(std::io::Error::other("scripted poll failure"));

    let error = driver.execute_cycle(&worker).unwrap_err();
    assert_eq!(error.to_string(), "scripted poll failure");

    worker.close_registrations();
    driver.shutdown(&poll_waker, &worker).unwrap();
    poll_waker.deactivate();
}

#[test]
fn primary_full_capacity_batch_yields_after_each_budget() {
    let poll = Poll::new().unwrap();
    let owner = StdArc::new(WakeCounter::default());
    let worker = Arc::new(WorkerState::new(
        std::thread::current().id(),
        poll.registry().try_clone().unwrap(),
        Waker::from(StdArc::clone(&owner)),
        Sink::noop(),
    ));
    let mut driver = PrimaryDriver::new(poll);
    driver.pending.extend(std::iter::repeat_n(wake_event(), EVENT_CAPACITY));

    for remaining in [768, 512, 256, 0] {
        driver.execute_cycle(&worker, Duration::ZERO).unwrap();
        assert_eq!(driver.pending.len(), remaining);
    }

    assert_eq!(owner.count(), 3);
}

#[test]
fn secondary_full_capacity_batch_resumes_after_the_last_budget() {
    let shared = SecondaryShared::default();
    {
        let mut state = shared.state();
        state.events.extend(std::iter::repeat_n(wake_event(), EVENT_CAPACITY));
        state.paused = true;
    }
    let mut events = Vec::with_capacity(COMPLETION_BUDGET);

    for remaining in [768, 512, 256] {
        let (_failure, resume, has_events) = drain_secondary_events(&shared, &mut events);
        assert_eq!(events.len(), COMPLETION_BUDGET);
        assert!(has_events);
        assert!(!resume);
        assert_eq!(shared.state().events.len(), remaining);
    }

    let (_failure, resume, has_events) = drain_secondary_events(&shared, &mut events);
    assert_eq!(events.len(), COMPLETION_BUDGET);
    assert!(!has_events);
    assert!(resume);
    assert!(!shared.state().paused);
}

fn wake_event() -> mio::event::Event {
    let mut poll = Poll::new().unwrap();
    let waker = mio::Waker::new(poll.registry(), Token(0)).unwrap();
    let mut events = Events::with_capacity(1);
    waker.wake().unwrap();
    poll.poll(&mut events, Some(Duration::ZERO)).unwrap();
    events.iter().next().unwrap().clone()
}

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl WakeCounter {
    fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

impl Wake for WakeCounter {
    fn wake(self: StdArc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn wake_by_ref(self: &StdArc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
