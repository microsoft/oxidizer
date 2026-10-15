// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Both synchronization backends use their public object's address as identity.

#![cfg(all(feature = "seismograph", not(miri)))]
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use performables::sync::barrier::Barrier;
use performables::sync::condition::Condvar;
use performables::sync::lock::RwLock;
use performables::sync::mode;
use performables::sync::mutex::Mutex;
use seismograph::recorder::event::{EventKind, ObjectId};
use seismograph::recorder::{Configuration, EventBufferCapacity, RecordingPolicy};

fn object_id<T>(object: &T) -> ObjectId {
    ObjectId::from_ptr(std::ptr::from_ref(object).cast::<()>())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one recording session covers both backends without global recorder interference"
)]
fn synchronization_modes_preserve_public_object_identity() {
    seismograph::recorder(Configuration {
        general_events: RecordingPolicy {
            enabled: true,
            ..Default::default()
        },
        event_capacity_per_thread: EventBufferCapacity::new(4_096).unwrap(),
        ..Default::default()
    });

    macro_rules! exercise {
        ($mode:ty) => {{
            // Keep each object allocated and alive through snapshot inspection,
            // so moves and address reuse cannot disguise an identity mismatch.
            let mutex = Box::new(Mutex::<u64, $mode>::new(7));
            let held = mutex.lock();
            assert!(mutex.try_lock().is_none());
            drop(held);
            catch_unwind(AssertUnwindSafe(|| {
                let _guard = mutex.lock();
                panic!("record poisoning");
            }))
            .unwrap_err();
            drop(mutex.lock_result().unwrap_err().into_inner());
            mutex.clear_poison();
            mutex.clear_poison();

            let lock = Box::new(RwLock::<u64, $mode>::new(11));
            let reader = lock.read();
            assert!(lock.try_write().is_none());
            drop(reader);
            let writer = lock.write();
            assert!(lock.try_read().is_none());
            drop(writer);

            let condition = Box::new(Condvar::<$mode>::new());
            let (guard, timeout) = condition.wait_timeout(mutex.lock(), Duration::ZERO);
            assert!(timeout.timed_out());
            drop(guard);
            condition.notify_one();
            condition.notify_all();
            *mutex.lock() = 0;
            std::thread::scope(|scope| {
                let (started_tx, started_rx) = std::sync::mpsc::channel();
                let wait_mutex = &*mutex;
                let wait_condition = &*condition;
                let worker = scope.spawn(move || {
                    let guard = wait_mutex.lock();
                    started_tx.send(()).unwrap();
                    drop(wait_condition.wait_while(guard, |value| *value == 0));
                });
                started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                *mutex.lock() = 1;
                condition.notify_one();
                worker.join().unwrap();
            });

            let barrier = Arc::new(Barrier::<$mode>::new(2));
            let other = Arc::clone(&barrier);
            let worker = std::thread::spawn(move || other.wait().is_leader());
            assert_ne!(barrier.wait().is_leader(), worker.join().unwrap());
            (mutex, lock, condition, barrier)
        }};
    }

    let synchronous = exercise!(mode::Sync);
    let asynchronous = exercise!(mode::Async);
    let encoded = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(encoded.as_bytes()).unwrap().events;

    macro_rules! identities {
        ($objects:expr) => {
            [
                object_id(&*$objects.0),
                object_id(&*$objects.1),
                object_id(&*$objects.2),
                object_id(&*$objects.3),
            ]
        };
    }
    let identities = [identities!(synchronous), identities!(asynchronous)];
    let expected: [&[EventKind]; 4] = [
        &[
            EventKind::MutexAccess,
            EventKind::MutexContention,
            EventKind::MutexRelease,
            EventKind::LockPoisoned,
            EventKind::LockPoisonObserved,
            EventKind::LockPoisonCleared,
        ],
        &[
            EventKind::RwLockReadAccess,
            EventKind::RwLockReadContention,
            EventKind::RwLockReadRelease,
            EventKind::RwLockWriteAccess,
            EventKind::RwLockWriteContention,
            EventKind::RwLockWriteRelease,
        ],
        &[EventKind::CondvarAccess, EventKind::CondvarContention, EventKind::CondvarNotify],
        &[EventKind::BarrierAccess, EventKind::BarrierContention, EventKind::BarrierRelease],
    ];
    for ids in &identities {
        for (id, kinds) in ids.iter().zip(expected) {
            for &kind in kinds {
                assert!(
                    snapshot
                        .events
                        .iter()
                        .any(|event| event.object_id() == Some(*id) && event.kind == kind),
                    "{kind:?} did not use public object identity {id:?}",
                );
            }
        }
        let poison_events = snapshot
            .events
            .iter()
            .filter(|event| event.object_id() == Some(ids[0]))
            .filter_map(|event| {
                matches!(
                    event.kind,
                    EventKind::LockPoisoned | EventKind::LockPoisonObserved | EventKind::LockPoisonCleared
                )
                .then_some(event.kind)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            poison_events,
            [EventKind::LockPoisoned, EventKind::LockPoisonObserved, EventKind::LockPoisonCleared],
        );
    }
    for event in &snapshot.events {
        if expected.iter().any(|kinds| kinds.contains(&event.kind)) {
            assert!(
                identities.iter().flatten().any(|id| event.object_id() == Some(*id)),
                "event used backend storage rather than a public object's identity: {event:?}",
            );
        }
    }
}
