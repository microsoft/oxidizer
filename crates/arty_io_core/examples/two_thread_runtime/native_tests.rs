// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Wake, Waker};
use std::thread;
use std::time::Duration;

use arty_io_core::{CompletionBudget, CompletionWaiter};

use super::{NativeRecord, NativeWaiter};

fn budget(units: usize) -> CompletionBudget {
    CompletionBudget::new(NonZeroUsize::new(units).unwrap())
}

#[derive(Default)]
struct CountWake(AtomicUsize);

impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        // This is an observation counter; event publication uses the adapter's queue locks.
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn zero_collection_preserves_the_latched_interruption() {
    let mut waiter = NativeWaiter::new();
    waiter.waker().wake_by_ref();
    waiter.collect(Duration::ZERO, &mut budget(1)).unwrap();
    assert!(*waiter.queue.latch.pending.lock().unwrap());
    waiter.collect(Duration::MAX, &mut budget(1)).unwrap();
    assert!(!*waiter.queue.latch.pending.lock().unwrap());
}

#[test]
fn exhausted_collection_never_waits_or_consumes_a_wake() {
    let mut waiter = NativeWaiter::new();
    let registration = waiter.record_client().register(Waker::noop().clone()).unwrap();
    registration.post(NativeRecord { token: 7, word: 9 }).unwrap();
    let mut exhausted = budget(1);
    assert!(exhausted.try_consume());
    assert!(waiter.collect(Duration::MAX, &mut exhausted).unwrap().is_runnable());
    assert!(*waiter.queue.latch.pending.lock().unwrap());
    assert!(!registration.has_records());
    waiter.collect(Duration::ZERO, &mut budget(1)).unwrap();
    assert_eq!(registration.pop().unwrap().token, 7);
    assert!(*waiter.queue.latch.pending.lock().unwrap());
}

#[test]
fn publish_in_the_last_scan_to_wait_gap_cannot_be_lost() {
    let mut waiter = NativeWaiter::new();
    let registration = Arc::new(waiter.record_client().register(Waker::noop().clone()).unwrap());
    let producer_registration = Arc::clone(&registration);
    let (publish_tx, publish_rx) = mpsc::channel();
    let (published_tx, published_rx) = mpsc::channel();
    let producer = thread::spawn(move || {
        publish_rx.recv().unwrap();
        producer_registration.post(NativeRecord { token: 1, word: 42 }).unwrap();
        published_tx.send(()).unwrap();
    });
    waiter.before_wait = Some(Box::new(move || {
        publish_tx.send(()).unwrap();
        // Publishing here also proves the collector did not retain a submitter's queue lock.
        published_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }));
    waiter.collect(Duration::MAX, &mut budget(1)).unwrap();
    assert_eq!(registration.pop().unwrap().word, 42);
    producer.join().unwrap();
}

#[test]
fn an_available_batch_is_bounded_and_is_not_filled_by_waiting() {
    let mut waiter = NativeWaiter::new();
    let registration = waiter.record_client().register(Waker::noop().clone()).unwrap();
    registration.post(NativeRecord { token: 1, word: 1 }).unwrap();
    registration.post(NativeRecord { token: 2, word: 2 }).unwrap();
    let mut first = budget(1);
    assert!(waiter.collect(Duration::MAX, &mut first).unwrap().is_runnable());
    assert_eq!(first.remaining(), 0);
    assert_eq!(registration.pop().unwrap().token, 1);
    let mut second = budget(8);
    assert!(!waiter.collect(Duration::MAX, &mut second).unwrap().is_runnable());
    assert_eq!(second.remaining(), 7);
    assert_eq!(registration.pop().unwrap().token, 2);
    assert!(!registration.has_records());
}

#[test]
fn an_ordinary_finite_timeout_is_idle() {
    let mut waiter = NativeWaiter::new();
    let status = waiter.collect(Duration::from_nanos(1), &mut budget(1)).unwrap();
    assert!(!status.is_runnable());
    assert_eq!(status.deadline(), None);
}

#[test]
fn retired_packets_cannot_target_a_new_registration() {
    let mut waiter = NativeWaiter::new();
    let old_wakes = Arc::new(CountWake::default());
    let new_wakes = Arc::new(CountWake::default());
    let old = waiter.record_client().register(Waker::from(Arc::clone(&old_wakes))).unwrap();
    old.post(NativeRecord { token: 0, word: 1 }).unwrap();
    old.retire();
    let new = waiter.record_client().register(Waker::from(Arc::clone(&new_wakes))).unwrap();
    waiter.collect(Duration::ZERO, &mut budget(2)).unwrap();
    assert!(!old.has_records());
    assert!(!new.has_records());
    // These diagnostic reads occur after synchronous collection on this thread.
    assert_eq!(old_wakes.0.load(Ordering::Relaxed), 0);
    assert_eq!(new_wakes.0.load(Ordering::Relaxed), 0);
    new.post(NativeRecord { token: 0, word: 99 }).unwrap();
    waiter.collect(Duration::ZERO, &mut budget(1)).unwrap();
    assert_eq!(new.pop().unwrap().word, 99);
    assert_eq!(new_wakes.0.load(Ordering::Relaxed), 1);
}

#[test]
fn late_readiness_and_domain_wakes_are_safe_after_retirement() {
    let mut waiter = NativeWaiter::new();
    let registration = waiter.readiness_client().register(Waker::noop().clone()).unwrap();
    registration.post().unwrap();
    registration.retire();
    waiter.collect(Duration::ZERO, &mut budget(1)).unwrap();
    assert!(!registration.take_ready());
    let waker = waiter.waker();
    drop(waiter);
    waker.wake_by_ref();
    assert!(registration.post().unwrap_err().to_string().contains("stopped"));
}
