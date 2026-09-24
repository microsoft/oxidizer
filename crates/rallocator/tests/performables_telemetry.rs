// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verifies the example dependency graph includes Performables runtime telemetry.

use performables::arc::Arc;
use seismograph::recorder::event::EventKind;
use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};

#[test]
fn example_dependency_graph_records_performables_events() {
    seismograph::recorder(seismograph::recorder::Configuration {
        general_events: seismograph::recorder::RecordingPolicy {
            enabled: true,
            ..Default::default()
        },
        arc_dereferences: seismograph::recorder::RecordingPolicy {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    });

    let value = Arc::new(7_u64);
    let object_id = Arc::telemetry_object_id(&value);
    std::hint::black_box(*value);

    let snapshot = seismograph::snapshot(SnapshotOptions {
        event_buffers: EventBufferDisposition::Release,
    })
    .unwrap();
    let events = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap().events;
    let kinds = events
        .events
        .iter()
        .filter(|event| event.object_id() == Some(object_id))
        .map(|event| event.kind)
        .collect::<Vec<_>>();

    assert_eq!(kinds, vec![EventKind::ArcCreate, EventKind::ArcDeref]);
    seismograph::recorder(seismograph::recorder::Configuration::default());
}
