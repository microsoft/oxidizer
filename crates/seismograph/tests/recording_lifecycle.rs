// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public lifecycle behavior when registered snapshot sources fail.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use seismograph::recorder::event::{EventClass, EventKind, ObjectId, Record};
use seismograph::recorder::{Configuration, EventBufferCapacity, Policy};
use seismograph::snapshot::{EventBufferDisposition, SnapshotContext, SnapshotOptions, Source, SourceData, SourceId};

static SOURCE_CALLS: AtomicUsize = AtomicUsize::new(0);
static FAILING_SOURCE: Source = Source::new(SourceId::new(1), "failing lifecycle source", 1, fail_source);

fn fail_source(_: SnapshotContext<'_>) -> Result<SourceData, seismograph::Error> {
    SOURCE_CALLS.fetch_add(1, Ordering::Relaxed);
    Err(seismograph::Error::new("source failed after recorder cleanup"))
}

#[test]
fn clear_skips_sources_and_a_later_source_failure_does_not_undo_stop() {
    seismograph::snapshot::register_source(&FAILING_SOURCE);
    let configuration = Configuration {
        general_events: Policy::all(false),
        cache: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64),
        ..Default::default()
    };
    seismograph::recorder(configuration);
    seismograph::record(EventClass::General, || {
        Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
    });
    seismograph::recorder::clear_buffers().unwrap();
    assert_eq!(SOURCE_CALLS.load(Ordering::Relaxed), 0);
    for class in [EventClass::General, EventClass::Cache] {
        let constructed = Cell::new(false);
        seismograph::record(class, || {
            constructed.set(true);
            None
        });
        assert!(constructed.get());
    }
    seismograph::record(EventClass::General, || {
        Some(Record::object(EventKind::MutexAccess, ObjectId::new(2)))
    });
    let error = seismograph::snapshot(SnapshotOptions {
        event_buffers: EventBufferDisposition::Stop,
    })
    .unwrap_err();
    assert!(error.to_string().contains("source failed after recorder cleanup"));
    assert_eq!(SOURCE_CALLS.load(Ordering::Relaxed), 1);
    for class in [EventClass::General, EventClass::Cache] {
        let constructed = Cell::new(false);
        seismograph::record(class, || {
            constructed.set(true);
            None
        });
        assert!(!constructed.get());
    }
}
