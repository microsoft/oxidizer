// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Recording changes do not alter native mutex ownership or poisoning.

#![cfg(not(miri))]

use std::panic::{AssertUnwindSafe, catch_unwind};

use performables::sync::mutex::Mutex;
use seismograph::recorder::event::{EventKind, ObjectId};
use seismograph::recorder::{Configuration, EventBufferCapacity, Policy};
use seismograph::snapshot::{SnapshotOptions, decode};

#[test]
fn enabling_recording_while_held_preserves_poison_and_release_events() {
    seismograph::recorder(Configuration::default());
    let mutex = Box::new(Mutex::<u64>::new(7));
    let id = ObjectId::from_ptr(std::ptr::from_ref(&*mutex).cast::<()>());
    let guard = mutex.lock();
    seismograph::recorder(Configuration {
        general_events: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64),
        ..Default::default()
    });
    catch_unwind(AssertUnwindSafe(move || {
        let _held = guard;
        panic!("unwind after recording is enabled");
    }))
    .unwrap_err();

    let mut recovered = mutex.lock_result().unwrap_err().into_inner();
    *recovered += 1;
    mutex.clear_poison();
    drop(recovered);
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    seismograph::recorder(Configuration::default());
    let events = decode(encoded.as_bytes())
        .unwrap()
        .events
        .events
        .into_iter()
        .filter(|event| event.object_id() == Some(id))
        .map(|event| event.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        (events, mutex.is_poisoned(), *mutex.lock()),
        (
            vec![
                EventKind::LockPoisoned,
                EventKind::MutexRelease,
                EventKind::MutexAccess,
                EventKind::LockPoisonObserved,
                EventKind::LockPoisonCleared,
                EventKind::MutexRelease,
            ],
            false,
            8,
        )
    );
}
