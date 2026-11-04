// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Ready blocking operations must not allocate waiter storage.

#![cfg(not(miri))]

use std::time::Duration;

use alloc_tracker::{Allocator, Session};
use performables::sync as mode;
use performables::sync::barrier::Barrier;
use performables::sync::channel::{bounded, oneshot, unbounded, watch};
use performables::sync::condition::Condvar;
use performables::sync::notify::Notify;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[test]
fn notification_coalescing_and_ready_waits_do_not_allocate() {
    let sync = Notify::<mode::Sync>::new();
    let asynchronous = Notify::<mode::Async>::new();
    let measured = measure_allocations(|| {
        sync.notify_one();
        sync.notify_one();
        sync.wait();
        asynchronous.notify_one();
        let mut wait = std::pin::pin!(asynchronous.wait_async());
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(wait.as_mut().poll(&mut cx).is_ready());
        assert!(!sync.wait_timeout(Duration::ZERO));
    });
    assert_eq!(measured, ((), 0));
}

#[expect(clippy::unwrap_used, reason = "a missing allocation measurement is a test harness failure")]
fn measure_allocations<T>(operation: impl FnOnce() -> T) -> (T, u64) {
    let session = Session::new().no_stdout().no_file();
    let measurement = session.operation("ready_blocking");
    let output = {
        let _span = measurement.measure_thread().iterations(1);
        operation()
    };
    let allocations = session.to_report().operations().next().unwrap().1.total_allocations_count();
    (output, allocations)
}

#[test]
fn ready_queue_send_and_receive_do_not_allocate() {
    for bounded_queue in [false, true] {
        let (sender, receiver) = if bounded_queue { bounded(1) } else { unbounded() };
        sender.try_send(0).unwrap();
        receiver.try_recv().unwrap();
        let measured = measure_allocations(|| (sender.send(7), receiver.recv()));

        assert_eq!(measured, ((Ok(()), Ok(7)), 0));
    }
}

#[test]
fn ready_queue_timeout_does_not_allocate_with_zero_or_overflowing_deadline() {
    let (sender, receiver) = bounded(1);
    sender.try_send(7).unwrap();
    let zero = measure_allocations(|| receiver.recv_timeout(Duration::ZERO));
    sender.try_send(11).unwrap();
    let overflow = measure_allocations(|| receiver.recv_timeout(Duration::MAX));

    assert_eq!((zero, overflow), ((Ok(7), 0), (Ok(11), 0)));
}

#[test]
fn ready_oneshot_receives_do_not_allocate() {
    let (sender, receiver) = oneshot();
    sender.send(7).unwrap();
    let (timed_sender, timed_receiver) = oneshot();
    timed_sender.send(11).unwrap();

    assert_eq!(
        (
            measure_allocations(|| receiver.recv()),
            measure_allocations(|| timed_receiver.recv_timeout(Duration::ZERO)),
        ),
        ((Ok(7), 0), (Ok(11), 0)),
    );
}

#[test]
fn ready_watch_change_and_closed_channels_do_not_allocate() {
    let (watch_sender, watch_receiver) = watch(0);
    watch_sender.send(7).unwrap();
    let changed = measure_allocations(|| watch_receiver.changed());
    drop(watch_sender);
    let closed_watch = measure_allocations(|| watch_receiver.changed().unwrap_err().is_closed());
    let (sender, receiver) = bounded::<u64>(1);
    assert!(receiver.close());
    let closed_queue = measure_allocations(|| {
        (
            sender.send(7).unwrap_err().is_closed(),
            receiver.recv().unwrap_err().is_closed(),
            receiver.recv_timeout(Duration::ZERO).unwrap_err().is_closed(),
        )
    });

    assert_eq!(
        (changed, closed_watch, closed_queue),
        ((Ok(()), 0), (true, 0), ((true, true, true), 0)),
    );
}

#[test]
fn warmed_watch_registration_and_cancellation_reuse_their_slot_without_allocating() {
    let (_sender, receiver) = watch(false);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    {
        let mut wait = std::pin::pin!(receiver.changed_async());
        assert!(wait.as_mut().poll(&mut context).is_pending());
    }
    let measured = measure_allocations(|| {
        for _ in 0..1_000 {
            let mut wait = std::pin::pin!(receiver.changed_async());
            assert!(wait.as_mut().poll(&mut context).is_pending());
        }
    });
    assert_eq!(measured, ((), 0));
}

#[test]
fn single_participant_barriers_and_empty_notifications_do_not_allocate() {
    let native = Barrier::<mode::Sync>::new(1);
    let asynchronous = Barrier::<mode::Async>::new(1);
    let condition = Condvar::<mode::Sync>::new();

    assert_eq!(
        measure_allocations(|| {
            condition.notify_one();
            condition.notify_all();
            (native.wait().is_leader(), asynchronous.wait().is_leader())
        }),
        ((true, true), 0),
    );
}
