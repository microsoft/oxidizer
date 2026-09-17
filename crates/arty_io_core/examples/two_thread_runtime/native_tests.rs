// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Wake, Waker};
use std::thread;
use std::time::Duration;

use arty_io_core::{CompletionBudget, CompletionWaiter, DriverContext, ServiceStatus};

use super::{NativeRecord, NativeWaiter, ReadinessClient, RecordClient};
use crate::test_support::Harness;

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
    waiter.collect(Duration::from_millis(10), &mut budget(1)).unwrap();
    assert!(!*waiter.queue.latch.pending.lock().unwrap());
}

#[test]
fn exhausted_collection_never_waits_or_consumes_a_wake() {
    let mut waiter = NativeWaiter::new();
    let registration = waiter.record_client().register(Waker::noop().clone()).unwrap();
    registration.post(NativeRecord { token: 7, word: 9 }).unwrap();
    let mut exhausted = budget(1);
    assert!(exhausted.try_consume());
    assert_eq!(
        waiter.collect(Duration::from_millis(10), &mut exhausted).unwrap(),
        ServiceStatus::Runnable
    );
    assert_eq!(waiter.wait_entries, 0);
    assert!(*waiter.queue.latch.pending.lock().unwrap());
    assert!(!registration.has_records());
    waiter.collect(Duration::ZERO, &mut budget(1)).unwrap();
    assert_eq!(registration.pop().unwrap().token, 7);
    assert!(*waiter.queue.latch.pending.lock().unwrap());
}

#[test]
fn exhausted_empty_collection_cannot_enter_the_native_wait() {
    let mut waiter = NativeWaiter::new();
    let mut exhausted = budget(1);
    assert!(exhausted.try_consume());
    assert_eq!(
        waiter.collect(Duration::from_millis(1), &mut exhausted).unwrap(),
        ServiceStatus::Idle
    );
    assert_eq!(waiter.wait_entries, 0);
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
    waiter.collect(Duration::from_millis(10), &mut budget(1)).unwrap();
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
    assert_eq!(
        waiter.collect(Duration::from_millis(10), &mut first).unwrap(),
        ServiceStatus::Runnable
    );
    assert_eq!(first.remaining(), 0);
    assert_eq!(registration.pop().unwrap().token, 1);
    let mut second = budget(8);
    assert_eq!(waiter.collect(Duration::from_millis(10), &mut second).unwrap(), ServiceStatus::Idle);
    assert_eq!(second.remaining(), 7);
    assert_eq!(registration.pop().unwrap().token, 2);
    assert!(!registration.has_records());
    assert_eq!(waiter.wait_entries, 0);
}

#[test]
fn an_ordinary_finite_timeout_is_idle() {
    let mut waiter = NativeWaiter::new();
    let status = waiter.collect(Duration::from_nanos(1), &mut budget(1)).unwrap();
    assert_eq!(status, ServiceStatus::Idle);
}

#[test]
fn a_registration_dropped_by_a_later_acquisition_failure_is_retired() {
    let mut waiter = NativeWaiter::new();
    let metrics = waiter.metrics();
    let client = waiter.record_client();
    let wakes = Arc::new(CountWake::default());
    {
        let acquired = client.register(Waker::from(Arc::clone(&wakes))).unwrap();
        acquired.post(NativeRecord { token: 3, word: 30 }).unwrap();
        // A second acquisition fails, so the successful one is dropped on the failure path.
        waiter.fail_next_record_registration();
        let failure = client.register(Waker::noop().clone()).unwrap_err();
        assert!(failure.to_string().contains("injected record registration failure"));
        // Diagnostic reads follow this thread's own registration calls.
        assert_eq!(metrics.records_created.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.records_retired.load(Ordering::Relaxed), 0);
    }
    assert_eq!(metrics.records_retired.load(Ordering::Relaxed), 1);

    // The in-flight packet retains the closed route: it is discarded, not left in an open mailbox.
    waiter.collect(Duration::ZERO, &mut budget(1)).unwrap();
    assert_eq!(metrics.records_retired.load(Ordering::Relaxed), 1);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
}

#[test]
fn a_readiness_registration_dropped_before_use_is_retired() {
    let mut waiter = NativeWaiter::new();
    let metrics = waiter.metrics();
    let wakes = Arc::new(CountWake::default());
    let registration = waiter.readiness_client().register(Waker::from(Arc::clone(&wakes))).unwrap();
    registration.post().unwrap();
    assert_eq!(metrics.readiness_retired.load(Ordering::Relaxed), 0);
    drop(registration);
    assert_eq!(metrics.readiness_retired.load(Ordering::Relaxed), 1);

    // A late readiness packet names the retired route and never signals a service participant.
    waiter.collect(Duration::ZERO, &mut budget(1)).unwrap();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    assert_eq!(metrics.readiness_retired.load(Ordering::Relaxed), 1);
}

#[test]
fn explicit_retirement_followed_by_drop_counts_once() {
    let waiter = NativeWaiter::new();
    let metrics = waiter.metrics();
    let record = waiter.record_client().register(Waker::noop().clone()).unwrap();
    let readiness = waiter.readiness_client().register(Waker::noop().clone()).unwrap();
    record.retire();
    record.retire();
    readiness.retire();
    readiness.retire();
    assert_eq!(metrics.records_retired.load(Ordering::Relaxed), 1);
    assert_eq!(metrics.readiness_retired.load(Ordering::Relaxed), 1);
    drop(record);
    drop(readiness);
    assert_eq!(metrics.records_retired.load(Ordering::Relaxed), 1);
    assert_eq!(metrics.readiness_retired.load(Ordering::Relaxed), 1);

    // Retirement counting is independent of the collector's own lifetime.
    drop(waiter);
    assert_eq!(metrics.records_created.load(Ordering::Relaxed), 1);
    assert_eq!(metrics.readiness_created.load(Ordering::Relaxed), 1);
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

#[test]
fn a_continuously_hot_source_cannot_starve_another_native_source() {
    let mut waiter = NativeWaiter::new();
    let hot = waiter.record_client().register(Waker::noop().clone()).unwrap();
    let other = waiter.readiness_client().register(Waker::noop().clone()).unwrap();
    for token in 0..2 {
        hot.post(NativeRecord { token, word: 0 }).unwrap();
    }
    other.post().unwrap();
    for turn in 0..6 {
        let mut allowance = budget(1);
        assert_eq!(waiter.collect(Duration::ZERO, &mut allowance).unwrap(), ServiceStatus::Runnable);
        assert_eq!(allowance.remaining(), 0);
        // Replenishment keeps A actionable throughout, including when B reaches the FIFO head.
        hot.post(NativeRecord { token: turn + 2, word: 0 }).unwrap();
        assert!(waiter.queue.has_events());
        assert_eq!(other.is_ready(), turn >= 2);
    }
    assert_eq!(waiter.wait_entries, 0);
    assert!(other.take_ready());
}

#[test]
fn each_attached_client_is_driven_by_its_actual_collector() {
    let harness = Harness::new(1);
    let mut first = NativeWaiter::new();
    let mut second = NativeWaiter::new();
    let context = |waiter: &NativeWaiter| {
        waiter
            .attach_clients(DriverContext::new(
                harness.thread.clone(),
                harness.tasks.handle(),
                Waker::noop().clone(),
            ))
            .unwrap()
    };
    let first_context = context(&first);
    let second_context = context(&second);
    let first_record = first_context
        .completion_service::<RecordClient>()
        .unwrap()
        .register(Waker::noop().clone())
        .unwrap();
    let second_record = second_context
        .completion_service::<RecordClient>()
        .unwrap()
        .register(Waker::noop().clone())
        .unwrap();
    let first_ready = first_context
        .completion_service::<ReadinessClient>()
        .unwrap()
        .register(Waker::noop().clone())
        .unwrap();
    let second_ready = second_context
        .completion_service::<ReadinessClient>()
        .unwrap()
        .register(Waker::noop().clone())
        .unwrap();
    first_record.post(NativeRecord { token: 1, word: 11 }).unwrap();
    second_record.post(NativeRecord { token: 2, word: 22 }).unwrap();
    first_ready.post().unwrap();
    second_ready.post().unwrap();
    first.collect(Duration::ZERO, &mut budget(2)).unwrap();
    assert_eq!(first_record.pop().unwrap().word, 11);
    assert!(first_ready.take_ready());
    assert!(!second_record.has_records());
    assert!(!second_ready.is_ready());
    second.collect(Duration::ZERO, &mut budget(2)).unwrap();
    assert_eq!(second_record.pop().unwrap().word, 22);
    assert!(second_ready.take_ready());
    drop(first);
    assert!(first_ready.post().is_err());
    second_ready.post().unwrap();
    second.collect(Duration::ZERO, &mut budget(1)).unwrap();
    assert!(second_ready.take_ready());
}
