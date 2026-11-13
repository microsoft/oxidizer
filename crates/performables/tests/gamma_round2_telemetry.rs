// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exact event counts for uncontended, contended, and terminal operations.

#![cfg(feature = "seismograph")]

use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use performables::sync::Async;
use performables::sync::barrier::Barrier;
use performables::sync::channel::{bounded, oneshot, watch};
use performables::sync::lock::RwLock;
use performables::sync::mutex::Mutex;
use performables::sync::notify::Notify;
use seismograph::recorder::event::EventKind;

fn count(kind: EventKind) -> usize {
    let thread = seismograph::recorder::thread_id();
    seismograph::snapshot::decode(
        seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default())
            .expect("the in-memory recorder snapshot must serialize successfully")
            .as_bytes(),
    )
    .expect("snapshots emitted by this recorder must decode")
    .events
    .events
    .iter()
    .filter(|event| event.thread_id == thread && event.kind == kind)
    .count()
}

fn record() {
    seismograph::recorder(seismograph::recorder::Configuration {
        general_events: seismograph::recorder::Policy::all(false),
        event_capacity_per_thread: seismograph::recorder::EventBufferCapacity::new(1024),
        ..Default::default()
    });
}

#[test]
fn gamma_round2_async_barrier_records_each_completed_participant_exactly_once() {
    record();
    let barrier = Barrier::<Async>::new(2);
    let before = count(EventKind::BarrierAccess);
    let mut first = barrier.wait_async();
    let mut second = barrier.wait_async();
    let mut context = Context::from_waker(Waker::noop());
    assert!(Pin::new(&mut first).poll(&mut context).is_pending());
    assert!(Pin::new(&mut second).poll(&mut context).is_ready());
    assert_eq!(count(EventKind::BarrierAccess), before + 1);
    assert!(Pin::new(&mut first).poll(&mut context).is_ready());
    assert_eq!(count(EventKind::BarrierAccess), before + 2);
}

#[test]
fn gamma_round2_registered_notification_records_only_one_successful_access() {
    record();
    let notify = Notify::default();
    let before = count(EventKind::NotifyAccess);
    let mut wait = notify.wait_async();
    let mut context = Context::from_waker(Waker::noop());
    assert!(Pin::new(&mut wait).poll(&mut context).is_pending());
    notify.notify_one();
    assert!(Pin::new(&mut wait).poll(&mut context).is_ready());
    assert_eq!(count(EventKind::NotifyAccess), before + 1);
    assert!(Pin::new(&mut wait).poll(&mut context).is_ready());
    assert_eq!(count(EventKind::NotifyAccess), before + 1);
}

#[test]
fn gamma_round2_uncontended_mutex_paths_do_not_report_contention() {
    record();
    let mutex = Mutex::<_, Async>::new(7);
    let before = count(EventKind::MutexContention);
    drop(mutex.lock());
    assert_eq!(count(EventKind::MutexContention), before);
    let mut future = mutex.lock_async();
    let Poll::Ready(guard) = Pin::new(&mut future).poll(&mut Context::from_waker(Waker::noop())) else {
        panic!("an uncontended mutex must be ready");
    };
    assert_eq!(*guard, 7);
    assert_eq!(count(EventKind::MutexContention), before);
}

#[test]
fn gamma_round2_uncontended_rwlock_paths_do_not_report_contention() {
    record();
    let lock = RwLock::<_, Async>::new(11);
    let before_read = count(EventKind::RwLockReadContention);
    let before_write = count(EventKind::RwLockWriteContention);
    drop(lock.read());
    drop(lock.write());
    let mut read = lock.read_async();
    let mut context = Context::from_waker(Waker::noop());
    let Poll::Ready(guard) = Pin::new(&mut read).poll(&mut context) else {
        panic!("an uncontended reader must be ready");
    };
    assert_eq!(*guard, 11);
    drop(guard);
    let mut write = lock.write_async();
    let Poll::Ready(guard) = Pin::new(&mut write).poll(&mut context) else {
        panic!("an uncontended writer must be ready");
    };
    assert_eq!(*guard, 11);
    assert_eq!(count(EventKind::RwLockReadContention), before_read);
    assert_eq!(count(EventKind::RwLockWriteContention), before_write);
}

#[test]
fn gamma_round2_native_try_lock_reports_contention_once() {
    record();
    let mutex = Mutex::<_>::new(13);
    let held = mutex.lock();
    let before = count(EventKind::MutexContention);
    assert!(mutex.try_lock().is_none());
    assert_eq!(count(EventKind::MutexContention), before + 1);
    drop(held);
}

#[test]
fn gamma_round2_try_send_reports_only_full_queue_contention() {
    record();
    let (sender, receiver) = bounded(1);
    let before = count(EventKind::ChannelSendContention);
    sender.try_send(17).unwrap();
    assert_eq!(count(EventKind::ChannelSendContention), before);
    assert!(sender.try_send(19).unwrap_err().is_full());
    assert_eq!(count(EventKind::ChannelSendContention), before + 1);
    drop(receiver);
    assert!(sender.try_send(23).unwrap_err().is_closed());
    assert_eq!(count(EventKind::ChannelSendContention), before + 1);
}

#[test]
fn gamma_round2_pending_oneshot_records_contention_once_and_drop_records_close() {
    record();
    let (_sender, mut receiver) = oneshot::<u32>();
    let before_contention = count(EventKind::ChannelReceiveContention);
    let before_close = count(EventKind::ChannelClose);
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..3 {
        assert!(Pin::new(&mut receiver).poll(&mut context).is_pending());
        assert_eq!(count(EventKind::ChannelReceiveContention), before_contention + 1);
    }
    drop(receiver);
    assert_eq!(count(EventKind::ChannelClose), before_close + 1);
}

#[test]
fn gamma_round2_watch_replace_and_each_borrow_emit_their_own_events() {
    record();
    let (sender, receiver) = watch(29);
    let before_send = count(EventKind::ChannelSend);
    let before_receive = count(EventKind::ChannelReceive);
    assert_eq!(sender.send_replace(31), 29);
    assert_eq!(count(EventKind::ChannelSend), before_send + 1);
    assert_eq!(*sender.borrow(), 31);
    assert_eq!(count(EventKind::ChannelReceive), before_receive + 1);
    assert_eq!(*receiver.borrow(), 31);
    assert_eq!(count(EventKind::ChannelReceive), before_receive + 2);
}

#[test]
fn gamma_round2_once_accesses_and_reinitialization_do_not_invent_contention() {
    use performables::sync::once::OnceLock;

    record();
    let mut cell = OnceLock::from(37);
    let contention = count(EventKind::OnceContention);
    assert_eq!(*cell.wait(), 37);
    assert_eq!(count(EventKind::OnceContention), contention);
    assert_eq!(cell.take(), Some(37));
    let access = count(EventKind::OnceAccess);
    let initialized = count(EventKind::OnceInitialize);
    assert_eq!(*cell.get_or_init(|| 41), 41);
    assert_eq!(count(EventKind::OnceAccess), access + 1);
    assert_eq!(count(EventKind::OnceInitialize), initialized + 1);
    assert_eq!(count(EventKind::OnceContention), contention);
}

#[test]
fn gamma_round2_condition_notification_before_poll_does_not_report_contention() {
    use performables::sync::condition::Condvar;

    record();
    let mutex = Mutex::<_, Async>::new(43);
    let condition = Condvar::<Async>::new();
    let mut wait = condition.wait_async(mutex.lock());
    condition.notify_one();
    let contention = count(EventKind::CondvarContention);
    let Poll::Ready(guard) = Pin::new(&mut wait).poll(&mut Context::from_waker(Waker::noop())) else {
        panic!("a notification before polling must permit immediate reacquisition");
    };
    assert_eq!(*guard, 43);
    assert_eq!(count(EventKind::CondvarContention), contention);
}
