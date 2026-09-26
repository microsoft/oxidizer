// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Coordinator interruption, token ownership, and cycle-transition races.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Wake, Waker};
use std::thread;
use std::time::{Duration, Instant};

use arty_io_core::{Coordinator, Cycle, PendingWork};
use static_assertions::{assert_impl_all, assert_not_impl_any};

assert_impl_all!(Coordinator: Send, Sync, std::fmt::Debug);
assert_not_impl_any!(Coordinator: Clone);
assert_impl_all!(PendingWork: Send, Sync, std::fmt::Debug);
assert_not_impl_any!(PendingWork: Clone);

#[derive(Default)]
struct Counter(AtomicUsize);

impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn interrupt_is_one_shot_and_late_registration_is_not_lost() {
    let mut coordinator = Coordinator::new();
    let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
    let mut token = cycle.start_work();
    let first = Arc::new(Counter::default());
    let late = Arc::new(Counter::default());
    token.on_interrupt(Waker::from(Arc::clone(&first)));

    let interrupt_waker = coordinator.interrupt_waker();
    interrupt_waker.wake_by_ref();
    interrupt_waker.wake_by_ref();
    token.on_interrupt(Waker::from(Arc::clone(&late)));

    assert_eq!(first.0.load(Ordering::Relaxed), 1);
    assert_eq!(late.0.load(Ordering::Relaxed), 1);
    assert!(token.is_interrupted());
    drop(token);
    coordinator.complete_cycle();
}

#[test]
fn external_interrupt_waker_wakes_registered_work() {
    let mut coordinator = Coordinator::new();
    coordinator.begin_cycle();
    let cycle = Cycle::new(Instant::now(), Duration::MAX, &coordinator);
    let mut work = cycle.start_work();
    let count = Arc::new(Counter::default());
    work.on_interrupt(Waker::from(Arc::clone(&count)));
    let external = coordinator.interrupt_waker();

    thread::spawn(move || external.wake()).join().unwrap();

    assert!(work.is_interrupted());
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
    drop(work);
    coordinator.complete_cycle();
}

#[test]
fn outstanding_work_blocks_cycle_completion_until_the_token_is_dropped() {
    let mut coordinator = Coordinator::new();
    coordinator.begin_cycle();
    let (completed, outstanding) = {
        let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
        (cycle.start_work(), cycle.start_work())
    };
    completed.complete();
    let (waiting_tx, waiting_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();

    let thread = thread::spawn(move || {
        waiting_tx.send(()).unwrap();
        coordinator.complete_cycle();
        done_tx.send(()).unwrap();
    });
    waiting_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let completed_early = done_rx.try_recv().is_ok();
    drop(outstanding);
    done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    thread.join().unwrap();

    assert!(!completed_early, "one completed token must not release another");
}

#[test]
fn dropping_pending_work_releases_the_completion_barrier() {
    let mut coordinator = Coordinator::new();
    coordinator.begin_cycle();
    let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
    let count = Arc::new(Counter::default());
    let mut first = cycle.start_work();
    first.on_interrupt(Waker::from(Arc::clone(&count)));
    let mut second = cycle.start_work();
    second.on_interrupt(Waker::from(Arc::clone(&count)));
    drop(first);
    drop(second);

    coordinator.complete_cycle();
    assert_eq!(
        count.0.load(Ordering::Relaxed),
        0,
        "wakers belonging to dropped work must be retired"
    );
    coordinator.begin_cycle();
}

#[test]
fn completed_work_interrupts_other_waiters_and_releases_the_barrier() {
    let mut coordinator = Coordinator::new();
    coordinator.begin_cycle();
    let count = Arc::new(Counter::default());
    let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
    let mut waiting = cycle.start_work();
    waiting.on_interrupt(Waker::from(Arc::clone(&count)));
    let completed = cycle.start_work();

    completed.complete();

    assert!(waiting.is_interrupted());
    drop(waiting);
    coordinator.complete_cycle();
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
}

#[test]
fn interruption_callback_can_release_other_pending_work() {
    struct DropWork(Mutex<Option<PendingWork>>);

    impl Wake for DropWork {
        fn wake(self: Arc<Self>) {
            drop(self.0.lock().unwrap().take());
        }
    }

    let mut coordinator = Coordinator::new();
    coordinator.begin_cycle();
    let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
    let released_by_callback = cycle.start_work();
    let mut waiting = cycle.start_work();
    waiting.on_interrupt(Waker::from(Arc::new(DropWork(Mutex::new(Some(released_by_callback))))));
    let completed = cycle.start_work();

    completed.complete();

    drop(waiting);
    coordinator.complete_cycle();
}

#[test]
fn begin_cycle_waits_for_work_from_the_previous_cycle() {
    let mut coordinator = Coordinator::new();
    coordinator.begin_cycle();
    let work = {
        let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
        cycle.start_work()
    };
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();

    let thread = thread::spawn(move || {
        started_tx.send(()).unwrap();
        coordinator.begin_cycle();
        done_tx.send(()).unwrap();
        coordinator
    });
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let completed_early = done_rx.try_recv().is_ok();
    drop(work);

    done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let mut coordinator = thread.join().unwrap();
    coordinator.complete_cycle();

    assert!(!completed_early, "begin_cycle must wait for work from the previous cycle");
}

#[test]
fn old_broadcast_finishing_after_cycle_transition_preserves_new_registrations() {
    struct HeldWake {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl Wake for HeldWake {
        fn wake(self: Arc<Self>) {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
        }
    }

    let mut coordinator = Coordinator::new();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (stable_waker, thread) = {
        let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
        let mut token = cycle.start_work();
        let stable_waker = coordinator.interrupt_waker();
        token.on_interrupt(Waker::from(Arc::new(HeldWake {
            entered: entered_tx,
            release: Mutex::new(release_rx),
        })));

        let broadcaster = stable_waker.clone();
        let thread = thread::spawn(move || broadcaster.wake());
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(token);
        (stable_waker, thread)
    };
    coordinator.begin_cycle();
    let next_cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
    let next = Arc::new(Counter::default());
    let mut next_token = next_cycle.start_work();
    next_token.on_interrupt(Waker::from(Arc::clone(&next)));
    release_tx.send(()).unwrap();
    thread.join().unwrap();

    assert_eq!(
        next.0.load(Ordering::Relaxed),
        0,
        "an old interrupt dispatch must not invoke new-cycle wakers"
    );
    stable_waker.wake_by_ref();
    assert_eq!(next.0.load(Ordering::Relaxed), 1);
    drop(next_token);
    coordinator.complete_cycle();
}
