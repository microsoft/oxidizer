// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Conditional drop recording preserves logical Arc lifetimes.

#![cfg(all(feature = "seismograph", not(miri)))]

use performables::arc::{Arc, PerThread};
use seismograph::recorder::event::{EventKind, ObjectId};
use seismograph::recorder::{Configuration, RecordingPolicy};
use seismograph::snapshot::{SnapshotOptions, decode};
use thread_aware::Relocator;

#[test]
fn only_the_last_logical_reference_records_a_drop() {
    let process = Arc::new(7_u64);
    let thread = Arc::<u64, PerThread>::new_with(|| 8);
    let (source, destination) = Relocator::between_threads().relocate(&mut ());
    let source = source.unwrap();
    let shared = Arc::new(9_u64);
    let aliased = Arc::<u64, PerThread>::try_from_values(&source, [(source.clone(), shared.clone()), (destination, shared)]).unwrap();
    let ids = [
        ObjectId::from_ptr(Arc::as_ptr(&process).cast::<()>()),
        ObjectId::from_ptr(Arc::as_ptr(&thread).cast::<()>()),
        ObjectId::from_ptr(Arc::as_ptr(&aliased).cast::<()>()),
    ];
    seismograph::recorder(Configuration {
        general_events: RecordingPolicy::all(false),
        ..Default::default()
    });

    drop(process.clone());
    drop(thread.clone());
    drop(aliased.clone());
    let before = seismograph::snapshot(SnapshotOptions::default()).unwrap();

    drop(process);
    drop(thread);
    drop(aliased);
    let after = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    seismograph::recorder(Configuration::default());

    let before = decode(before.as_bytes()).unwrap().events.events;
    let after = decode(after.as_bytes()).unwrap().events.events;
    assert_eq!(
        (
            before.iter().map(|event| (event.kind, event.object_id())).collect::<Vec<_>>(),
            after.iter().map(|event| (event.kind, event.object_id())).collect::<Vec<_>>(),
        ),
        (
            ids.map(|id| (EventKind::ArcClone, Some(id))).to_vec(),
            [
                (EventKind::ArcClone, Some(ids[0])),
                (EventKind::ArcClone, Some(ids[1])),
                (EventKind::ArcClone, Some(ids[2])),
                (EventKind::ArcDrop, Some(ids[0])),
                (EventKind::ArcDrop, Some(ids[1])),
                (EventKind::ArcDrop, Some(ids[2])),
            ]
            .to_vec(),
        )
    );
}
