// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exact event counts for repeated polls, successful completion and final closure.

#![cfg(feature = "seismograph")]

use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use performables::sync::barrier::Barrier;
use performables::sync::channel::{bounded, oneshot, watch};
use performables::sync::condition::Condvar;
use performables::sync::lock::RwLock;
use performables::sync::mutex::Mutex;
use performables::sync::notify::Notify;
use performables::sync::once::OnceLock;
use performables::sync::{Async, Sync};
use seismograph::recorder::event::EventKind;
use seismograph::recorder::{Configuration, EventBufferCapacity, Policy};
use seismograph::snapshot::{SnapshotOptions, decode};

#[path = "support/waker.rs"]
mod waker_support;

static RECORDING_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn nov4_notification_during_waker_registration_records_exactly_one_success() {
    let _recording = RECORDING_LOCK.lock().unwrap();
    seismograph::recorder(Configuration {
        general_events: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(1024),
        ..Default::default()
    });
    let notify = std::sync::Arc::new(Notify::<Async>::new());
    let signal = std::sync::Arc::clone(&notify);
    let waker = waker_support::clone_hook_waker(move || signal.notify_one());
    let mut cx = Context::from_waker(&waker);
    let mut wait = Box::pin(notify.wait_async());
    delta(EventKind::NotifyAccess, 1, || {
        assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(()));
    });
    delta(EventKind::NotifyAccess, 0, || {
        assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(()));
    });
    drop(wait);
    assert!(!notify.try_wait());
    seismograph::recorder(Configuration::default());
}

#[test]
fn nov4_barrier_follower_records_access_only_when_its_generation_completes() {
    let _recording = RECORDING_LOCK.lock().unwrap();
    seismograph::recorder(Configuration {
        general_events: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(1024),
        ..Default::default()
    });
    let barrier = std::sync::Arc::new(Barrier::<Async>::new(2));
    let mut first = Box::pin(barrier.wait_async());
    let mut cx = Context::from_waker(Waker::noop());
    delta(EventKind::BarrierAccess, 0, || {
        assert!(first.as_mut().poll(&mut cx).is_pending());
    });
    let after_leader = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(usize::MAX));
    let observed = std::sync::Arc::clone(&after_leader);
    let second_barrier = std::sync::Arc::clone(&barrier);
    let waker = waker_support::clone_hook_waker(move || {
        let mut leader = Box::pin(second_barrier.wait_async());
        let Poll::Ready(outcome) = leader.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
            panic!("the second arrival must complete the generation");
        };
        assert!(outcome.is_leader());
        observed.store(count(EventKind::BarrierAccess), std::sync::atomic::Ordering::Relaxed);
    });
    let Poll::Ready(outcome) = first.as_mut().poll(&mut Context::from_waker(&waker)) else {
        panic!("registration must recheck the generation changed by its waker clone");
    };
    assert!(!outcome.is_leader());
    assert_eq!(
        count(EventKind::BarrierAccess),
        after_leader.load(std::sync::atomic::Ordering::Relaxed) + 1
    );
    seismograph::recorder(Configuration::default());
}

#[cfg(test)]
fn count(kind: EventKind) -> usize {
    let thread = seismograph::recorder::thread_id();
    decode(seismograph::snapshot(SnapshotOptions::default()).unwrap().as_bytes())
        .unwrap()
        .events
        .events
        .iter()
        .filter(|event| event.thread_id == thread && event.kind == kind)
        .count()
}

#[cfg(test)]
fn delta(kind: EventKind, expected: usize, operation: impl FnOnce()) {
    let before = count(kind);
    operation();
    assert_eq!(count(kind) - before, expected, "{kind:?}");
}

#[test]
fn gamma_survivor_telemetry_records_one_contention_and_only_actual_state_transitions() {
    let _recording = RECORDING_LOCK.lock().unwrap();
    seismograph::recorder(Configuration {
        general_events: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(1024),
        ..Default::default()
    });
    ready_lock_and_cell_events();
    notification_and_barrier_events();
    lock_contention_events();
    channel_events();
    seismograph::recorder(Configuration::default());
}

#[cfg(test)]
fn ready_lock_and_cell_events() {
    let native_mutex = Mutex::<u32, Sync>::new(7);
    delta(EventKind::MutexAccess, 1, || assert_eq!(*native_mutex.try_lock().unwrap(), 7));
    delta(EventKind::MutexRelease, 1, || drop(native_mutex.lock()));
    let native_rwlock = RwLock::<u32, Sync>::new(7);
    delta(EventKind::RwLockReadAccess, 1, || assert_eq!(*native_rwlock.try_read().unwrap(), 7));
    delta(EventKind::RwLockReadRelease, 1, || drop(native_rwlock.read()));
    delta(EventKind::RwLockWriteAccess, 1, || {
        assert_eq!(*native_rwlock.try_write().unwrap(), 7);
    });
    delta(EventKind::RwLockWriteRelease, 1, || drop(native_rwlock.write()));
    let poisoned = RwLock::<u32, Async>::new(7);
    delta(EventKind::LockPoisoned, 1, || {
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = poisoned.write();
                panic!("controlled writer failure");
            }))
            .is_err()
        );
    });
    assert!(poisoned.is_poisoned());
    delta(EventKind::LockPoisonObserved, 1, || {
        poisoned.try_read_result().unwrap_err();
    });
    delta(EventKind::LockPoisonObserved, 1, || {
        poisoned.try_write_result().unwrap_err();
    });
    delta(EventKind::LockPoisonCleared, 1, || poisoned.clear_poison());
    delta(EventKind::LockPoisonCleared, 0, || poisoned.clear_poison());
    assert!(!poisoned.is_poisoned());
    let poisoned_mutex = Mutex::<u32, Async>::new(7);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = poisoned_mutex.lock();
            panic!("controlled mutex failure");
        }))
        .is_err()
    );
    delta(EventKind::LockPoisonCleared, 1, || poisoned_mutex.clear_poison());
    delta(EventKind::LockPoisonCleared, 0, || poisoned_mutex.clear_poison());
    let cell = OnceLock::new();
    delta(EventKind::OnceAccess, 0, || assert_eq!(cell.get(), None));
    delta(EventKind::OnceInitialize, 1, || assert_eq!(cell.set(7), Ok(())));
    delta(EventKind::OnceInitialize, 0, || assert_eq!(cell.set(11), Err(11)));
    delta(EventKind::OnceAccess, 2, || {
        assert_eq!(cell.get(), Some(&7));
        assert_eq!(cell.wait(), &7);
    });
    let empty = OnceLock::<u32>::new();
    delta(EventKind::OnceContention, 0, || assert_eq!(empty.get_or_init(|| 13), &13));
}

#[cfg(test)]
fn notification_and_barrier_events() {
    let mut cx = Context::from_waker(Waker::noop());
    let notify = Notify::<Async>::new();
    let native_notify = Notify::<Sync>::new();
    delta(EventKind::NotifyContention, 1, || {
        assert!(!native_notify.wait_timeout(std::time::Duration::ZERO));
    });
    notify.notify_one();
    delta(EventKind::NotifyAccess, 1, || assert!(notify.try_wait()));
    delta(EventKind::NotifyAccess, 0, || assert!(!notify.try_wait()));
    let mut notified = Box::pin(notify.wait_async());
    delta(EventKind::NotifyContention, 1, || {
        for _ in 0..3 {
            assert!(notified.as_mut().poll(&mut cx).is_pending());
        }
    });
    notify.notify_one();
    delta(EventKind::NotifyAccess, 1, || assert!(notified.as_mut().poll(&mut cx).is_ready()));
    delta(EventKind::NotifyAccess, 0, || assert!(notified.as_mut().poll(&mut cx).is_ready()));
    drop(notified);
    let condition = Condvar::<Async>::new();
    delta(EventKind::CondvarNotify, 2, || {
        condition.notify_one();
        condition.notify_all();
    });
    let condition_mutex = Mutex::<u32, Async>::new(7);
    let mut condition_wait = Box::pin(condition.wait_async(condition_mutex.lock()));
    delta(EventKind::CondvarContention, 1, || {
        for _ in 0..3 {
            assert!(condition_wait.as_mut().poll(&mut cx).is_pending());
        }
    });
    condition.notify_one();
    delta(EventKind::CondvarAccess, 1, || {
        assert!(condition_wait.as_mut().poll(&mut cx).is_ready());
    });
    drop(condition_wait);
    for _ in 0..3 {
        let barrier = Barrier::<Sync>::new(1);
        delta(EventKind::BarrierAccess, 1, || assert!(barrier.wait().is_leader()));
        let asynchronous = Barrier::<Async>::new(2);
        let mut first = Box::pin(asynchronous.wait_async());
        delta(EventKind::BarrierContention, 1, || {
            assert!(first.as_mut().poll(&mut cx).is_pending());
        });
        let mut second = Box::pin(asynchronous.wait_async());
        delta(EventKind::BarrierAccess, 2, || {
            assert!(second.as_mut().poll(&mut cx).is_ready());
            assert!(first.as_mut().poll(&mut cx).is_ready());
        });
    }
}

#[cfg(test)]
fn lock_contention_events() {
    let mut cx = Context::from_waker(Waker::noop());
    let mutex = Mutex::<u32, Async>::new(7);
    let held = mutex.lock();
    let mut waiting = Box::pin(mutex.lock_async());
    delta(EventKind::MutexContention, 1, || {
        for _ in 0..3 {
            assert!(waiting.as_mut().poll(&mut cx).is_pending());
        }
    });
    drop(held);
    let Poll::Ready(held) = waiting.as_mut().poll(&mut cx) else {
        panic!("released mutex must acquire");
    };
    drop((held, waiting));
    let lock = RwLock::<u32, Async>::new(11);
    let held = lock.write();
    let mut read = Box::pin(lock.read_async());
    delta(EventKind::RwLockReadContention, 1, || {
        for _ in 0..3 {
            assert!(read.as_mut().poll(&mut cx).is_pending());
        }
    });
    drop(held);
    let Poll::Ready(held) = read.as_mut().poll(&mut cx) else {
        panic!("released writer must allow a reader");
    };
    let mut write = Box::pin(lock.write_async());
    delta(EventKind::RwLockWriteContention, 1, || {
        for _ in 0..3 {
            assert!(write.as_mut().poll(&mut cx).is_pending());
        }
    });
    drop(held);
    let Poll::Ready(held) = write.as_mut().poll(&mut cx) else {
        panic!("released reader must allow a writer");
    };
    drop((held, read, write));
}

#[cfg(test)]
fn channel_events() {
    let mut cx = Context::from_waker(Waker::noop());
    let (sender, receiver) = bounded(1);
    sender.try_send(7).unwrap();
    let mut send = Box::pin(sender.send_async(11));
    delta(EventKind::ChannelSendContention, 1, || {
        for _ in 0..3 {
            assert!(send.as_mut().poll(&mut cx).is_pending());
        }
    });
    assert_eq!(receiver.try_recv(), Ok(7));
    assert_eq!(send.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    drop(send);
    assert_eq!(receiver.try_recv(), Ok(11));
    let mut receive = Box::pin(receiver.recv_async());
    delta(EventKind::ChannelReceiveContention, 1, || {
        for _ in 0..3 {
            assert!(receive.as_mut().poll(&mut cx).is_pending());
        }
    });
    drop(receive);
    let additional = sender.clone();
    delta(EventKind::ChannelClose, 0, || drop(additional));
    delta(EventKind::ChannelClose, 1, || drop(sender));
    drop(receiver);
    let (sender, receiver) = bounded::<u32>(1);
    let additional = receiver.clone();
    delta(EventKind::ChannelClose, 0, || drop(additional));
    delta(EventKind::ChannelClose, 1, || drop(receiver));
    delta(EventKind::ChannelClose, 1, || drop(sender));
    let (sender, receiver) = bounded::<u32>(1);
    delta(EventKind::ChannelClose, 1, || assert!(sender.close()));
    delta(EventKind::ChannelClose, 0, || assert!(!sender.close()));
    drop((sender, receiver));
    for sent in [false, true] {
        let (sender, mut receiver) = oneshot();
        delta(EventKind::ChannelReceiveContention, 1, || {
            for _ in 0..3 {
                assert!(receiver.try_recv().unwrap_err().is_empty());
                assert!(Pin::new(&mut receiver).poll(&mut cx).is_pending());
            }
        });
        if sent {
            delta(EventKind::ChannelClose, 0, || sender.send(13).unwrap());
            delta(EventKind::ChannelClose, 1, || assert_eq!(receiver.try_recv(), Ok(13)));
        } else {
            delta(EventKind::ChannelClose, 1, || drop(sender));
            assert!(receiver.try_recv().unwrap_err().is_closed());
        }
        delta(EventKind::ChannelClose, 0, || drop(receiver));
    }
    let (sender, receiver) = watch(0);
    delta(EventKind::ChannelSend, 1, || sender.send(17).unwrap());
    delta(EventKind::ChannelReceive, 1, || assert_eq!(*receiver.borrow_and_update(), 17));
    delta(EventKind::ChannelSend, 1, || {
        sender.send_modify(|value| {
            *value = 19;
        });
    });
    delta(EventKind::ChannelReceive, 1, || assert_eq!(receiver.changed(), Ok(())));
    delta(EventKind::ChannelReceive, 1, || {
        let mut wait = Box::pin(receiver.wait_for_async(|value| *value == 19));
        assert!(wait.as_mut().poll(&mut cx).is_ready());
    });
    let mut wait = Box::pin(receiver.changed_async());
    delta(EventKind::ChannelReceiveContention, 1, || {
        for _ in 0..3 {
            assert!(wait.as_mut().poll(&mut cx).is_pending());
        }
    });
    sender.send(23).unwrap();
    assert_eq!(wait.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    drop(wait);
    delta(EventKind::ChannelClose, 1, || drop(sender));
    drop(receiver);
}
