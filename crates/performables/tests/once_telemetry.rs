// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Initialized cells preserve unused-initializer destruction before access.

#![cfg(not(miri))]

use performables::sync::once::OnceLock;
use seismograph::recorder::event::{EventClass, EventKind, ObjectId, Record};
use seismograph::recorder::{Configuration, EventBufferCapacity, RecordingPolicy};
use seismograph::snapshot::{SnapshotOptions, decode};

struct RecordOnDrop(ObjectId);

impl Drop for RecordOnDrop {
    fn drop(&mut self) {
        seismograph::record(EventClass::General, || Some(Record::object(EventKind::ArcDrop, self.0)));
    }
}

#[test]
fn initialized_access_drops_the_unused_initializer_before_recording_access() {
    let once = Box::new(OnceLock::from(7_u64));
    let id = ObjectId::from_ptr(std::ptr::from_ref(&*once).cast::<()>());
    let captured = RecordOnDrop(id);
    seismograph::recorder(Configuration {
        general_events: RecordingPolicy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
        ..Default::default()
    });
    let value = *once.get_or_init(move || {
        drop(captured);
        panic!("an initialized cell must not invoke its initializer");
    });
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
    assert_eq!((value, events), (7, vec![EventKind::ArcDrop, EventKind::OnceAccess]));
}

testing_aids::init_tracing!();
