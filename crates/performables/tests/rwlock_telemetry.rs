// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Recording enabled after acquisition preserves native lock events.
//! One test owns the process-wide recorder configuration.

#![cfg(not(miri))]
#![expect(clippy::unwrap_used, reason = "test helpers use unwrap for failure backtraces")]

use std::panic::{AssertUnwindSafe, catch_unwind};

use performables::sync::lock::RwLock;
use seismograph::recorder::event::{EventKind, ObjectId};
use seismograph::recorder::{Configuration, EventBufferCapacity, RecordingPolicy};
use seismograph::snapshot::{SnapshotOptions, decode};

fn enable_recording() {
    seismograph::recorder(Configuration {
        general_events: RecordingPolicy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
        ..Default::default()
    });
}

fn finish_recording(id: ObjectId) -> Vec<EventKind> {
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    seismograph::recorder(Configuration::default());
    decode(encoded.as_bytes())
        .unwrap()
        .events
        .events
        .into_iter()
        .filter(|event| event.object_id() == Some(id))
        .map(|event| event.kind)
        .collect()
}

#[test]
fn recording_enabled_while_held_preserves_release_and_poison_events() {
    seismograph::recorder(Configuration::default());
    let read_lock = Box::new(RwLock::<u64>::new(7));
    let write_lock = Box::new(RwLock::<u64>::new(7));
    let read_id = ObjectId::from_ptr(std::ptr::from_ref(&*read_lock).cast::<()>());
    let write_id = ObjectId::from_ptr(std::ptr::from_ref(&*write_lock).cast::<()>());
    let reader = read_lock.read();
    enable_recording();
    drop(reader);
    let read_events = finish_recording(read_id);

    let writer = write_lock.write();
    enable_recording();
    catch_unwind(AssertUnwindSafe(move || {
        let _held = writer;
        panic!("unwind after recording is enabled");
    }))
    .unwrap_err();

    drop(write_lock.read_result().unwrap_err().into_inner());
    let mut recovered = write_lock.write_result().unwrap_err().into_inner();
    *recovered += 1;
    write_lock.clear_poison();
    drop(recovered);
    let write_events = finish_recording(write_id);
    assert_eq!(
        (
            read_events,
            *read_lock.read(),
            write_events,
            write_lock.is_poisoned(),
            *write_lock.read()
        ),
        (
            vec![EventKind::RwLockReadRelease],
            7,
            vec![
                EventKind::LockPoisoned,
                EventKind::RwLockWriteRelease,
                EventKind::RwLockReadAccess,
                EventKind::LockPoisonObserved,
                EventKind::RwLockReadRelease,
                EventKind::RwLockWriteAccess,
                EventKind::LockPoisonObserved,
                EventKind::LockPoisonCleared,
                EventKind::RwLockWriteRelease,
            ],
            false,
            8,
        )
    );
}

testing_aids::init_tracing!();
