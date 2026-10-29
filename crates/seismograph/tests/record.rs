// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public event recording, source observations, and scoped suppression.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, MutexGuard};

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp, ObjectId, Record};
use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId};
use seismograph::recorder::{Configuration, EventBufferCapacity, Policy, generation, is_enabled, try_record, without_recording};
use seismograph::snapshot::{self, EventBufferDisposition, SnapshotContext, SnapshotOptions, Source, SourceData, SourceId};

static RECORDING: Mutex<()> = Mutex::new(());

struct Recording {
    _lock: MutexGuard<'static, ()>,
}

impl Recording {
    #[expect(clippy::unwrap_used, reason = "test fixtures require an unpoisoned recorder lock")]
    fn start(configuration: Configuration) -> Self {
        let lock = RECORDING.lock().unwrap();
        seismograph::recorder(Configuration {
            event_capacity_per_thread: EventBufferCapacity::new(64),
            ..configuration
        });
        Self { _lock: lock }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        seismograph::recorder(Configuration::default());
    }
}

fn object(kind: EventKind) -> Record {
    Record::object(kind, ObjectId::new(1))
}

#[test]
fn event_capacity_accepts_bounded_powers_of_two_including_constants() {
    const MINIMUM: EventBufferCapacity = EventBufferCapacity::new(64);
    const MAXIMUM: EventBufferCapacity = EventBufferCapacity::new(1_048_576);

    assert_eq!(
        (
            MINIMUM.get(),
            EventBufferCapacity::new(128).get(),
            EventBufferCapacity::new(65_536).get(),
            MAXIMUM.get(),
            EventBufferCapacity::try_from(64).unwrap(),
            EventBufferCapacity::try_from(1_048_576).unwrap(),
        ),
        (64, 128, 65_536, 1_048_576, MINIMUM, MAXIMUM)
    );
}

#[test]
#[should_panic(expected = "event buffer capacity must be a power of two between 64 and 1048576")]
fn event_capacity_rejects_zero() {
    let _ = EventBufferCapacity::new(0);
}

#[test]
#[should_panic(expected = "event buffer capacity must be a power of two between 64 and 1048576")]
fn event_capacity_rejects_a_power_of_two_below_the_minimum() {
    let _ = EventBufferCapacity::new(32);
}

#[test]
#[should_panic(expected = "event buffer capacity must be a power of two between 64 and 1048576")]
fn event_capacity_rejects_a_non_power_of_two() {
    let _ = EventBufferCapacity::new(1_000);
}

#[test]
#[should_panic(expected = "event buffer capacity must be a power of two between 64 and 1048576")]
fn event_capacity_rejects_a_power_of_two_above_the_maximum() {
    let _ = EventBufferCapacity::new(2_097_152);
}

#[test]
#[should_panic(expected = "event buffer capacity must be a power of two between 64 and 1048576")]
fn event_capacity_rejects_the_largest_integer() {
    let _ = EventBufferCapacity::new(usize::MAX);
}

#[test]
fn invalid_event_capacity_conversion_returns_an_error_without_panicking() {
    assert_eq!(
        EventBufferCapacity::try_from(1_000).unwrap_err().to_string(),
        "event buffer capacity must be a power of two between 64 and 1048576"
    );
}

#[test]
fn record_checks_categories_and_try_record_reports_the_accepting_generation() {
    let _recording = Recording::start(Configuration {
        general_events: Policy::all(false),
        arc_dereferences: Policy::all(false),
        ..Default::default()
    });
    let recording_generation = generation().unwrap();

    seismograph::record(EventClass::General, || Some(object(EventKind::MutexAccess)));
    let accepted = try_record(EventClass::ArcDereference, || Some(object(EventKind::ArcDeref)));
    seismograph::record(EventClass::General, || Some(object(EventKind::ArcDeref)));
    let rejected = try_record(EventClass::ArcDereference, || Some(object(EventKind::MutexRelease)));
    seismograph::record(EventClass::General, || Some(object(EventKind::MutexRelease)));

    let captured = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = snapshot::decode(captured.as_bytes()).unwrap();
    assert_eq!(
        (
            accepted,
            rejected,
            generation(),
            decoded.events.events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        ),
        (
            Some(recording_generation),
            None,
            Some(recording_generation),
            vec![EventKind::MutexAccess, EventKind::ArcDeref, EventKind::MutexRelease],
        )
    );
}

#[test]
fn generation_is_process_wide_and_independent_of_thread_suppression() {
    let _recording = Recording::start(Configuration {
        general_events: Policy::all(false),
        ..Default::default()
    });
    let active = generation().unwrap();
    let observe = || (generation(), is_enabled(EventClass::General), is_enabled(EventClass::Allocation));

    let before = observe();
    let suppressed = without_recording(observe);
    let after = observe();

    assert_eq!(
        (before, suppressed, after),
        (
            (Some(active), true, false),
            (Some(active), false, false),
            (Some(active), true, false)
        )
    );
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))] // Disabled and stopped recording deliberately skips these builders.
fn disabled_and_stopped_recording_skip_builders_and_restarts_change_generation() {
    let _recording = Recording::start(Configuration::default());
    let constructed = Cell::new(0);
    let build = || {
        constructed.set(constructed.get() + 1);
        Some(object(EventKind::MutexAccess))
    };
    assert!(generation().is_none());
    assert!(!is_enabled(EventClass::General));
    seismograph::record(EventClass::General, build);
    assert!(try_record(EventClass::General, build).is_none());
    assert_eq!(constructed.get(), 0);

    let configuration = Configuration {
        general_events: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64),
        ..Default::default()
    };
    seismograph::recorder(configuration);
    assert!(is_enabled(EventClass::General));
    let first = try_record(EventClass::General, build).unwrap();
    seismograph::recorder(configuration);
    assert_eq!(generation(), Some(first));

    seismograph::recorder::clear_buffers().unwrap();
    let second = generation().unwrap();
    assert_ne!(first, second);
    assert_eq!(constructed.get(), 1);
    seismograph::record(EventClass::General, build);
    assert_eq!(constructed.get(), 2);

    let _capture = seismograph::snapshot(SnapshotOptions {
        event_buffers: EventBufferDisposition::Stop,
    })
    .unwrap();
    assert!(generation().is_none());
    seismograph::record(EventClass::General, build);
    assert!(try_record(EventClass::General, build).is_none());
    assert_eq!(constructed.get(), 2);
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))] // Suppression deliberately prevents the builders from running.
fn suppression_is_nested_thread_local_and_restored_after_unwinding() {
    let _recording = Recording::start(Configuration {
        general_events: Policy::all(false),
        ..Default::default()
    });
    let recording_generation = generation().unwrap();
    let constructed = Cell::new(false);

    without_recording(|| {
        assert_eq!(generation(), Some(recording_generation));
        assert!(!is_enabled(EventClass::General));
        seismograph::record(EventClass::General, || {
            constructed.set(true);
            Some(object(EventKind::MutexAccess))
        });
        assert!(
            try_record(EventClass::General, || {
                constructed.set(true);
                Some(object(EventKind::MutexAccess))
            })
            .is_none()
        );
        without_recording(|| assert!(!is_enabled(EventClass::General)));
        assert!(!is_enabled(EventClass::General));
        assert_eq!(std::thread::spawn(generation).join().unwrap(), Some(recording_generation));
    });
    assert!(!constructed.get());
    assert!(is_enabled(EventClass::General));
    assert_eq!(generation(), Some(recording_generation));

    let panic = catch_unwind(AssertUnwindSafe(|| {
        without_recording(|| panic!("test suppression unwinding"));
    }));
    assert!(panic.is_err());
    assert_eq!(generation(), Some(recording_generation));
    seismograph::record(EventClass::General, || Some(object(EventKind::MutexAccess)));
    let captured = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = snapshot::decode(captured.as_bytes()).unwrap();
    assert_eq!(
        decoded.events.events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [EventKind::MutexAccess]
    );
}

#[test]
fn querying_or_skipping_events_does_not_initialize_event_buffers() {
    let _recording = Recording::start(Configuration {
        general_events: Policy::all(false),
        ..Default::default()
    });
    assert!(generation().is_some());
    seismograph::record(EventClass::General, || None);
    assert!(try_record(EventClass::General, || None).is_none());

    let captured = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = snapshot::decode(captured.as_bytes()).unwrap();
    assert_eq!(
        (
            decoded.events.total_events,
            decoded.events.events.len(),
            decoded.events.threads.len()
        ),
        (0, 0, 0)
    );
}

#[test]
fn recording_changes_inside_builders_reject_the_old_event() {
    let configuration = Configuration {
        general_events: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64),
        ..Default::default()
    };
    let _recording = Recording::start(configuration);
    seismograph::record(EventClass::General, || {
        seismograph::recorder(Configuration {
            event_capacity_per_thread: EventBufferCapacity::new(128),
            ..configuration
        });
        Some(object(EventKind::MutexAccess))
    });

    assert!(
        try_record(EventClass::General, || {
            seismograph::recorder(configuration);
            Some(object(EventKind::MutexAccess))
        })
        .is_none()
    );
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = snapshot::decode(encoded.as_bytes()).unwrap();
    assert_eq!((decoded.events.total_events, decoded.events.events.len()), (0, 0));
}

#[test]
fn event_backtraces_follow_the_category_policy() {
    let _recording = Recording::start(Configuration {
        general_events: Policy::all(true),
        runtime_tasks: Policy::all(false),
        ..Default::default()
    });
    seismograph::record(EventClass::General, || Some(object(EventKind::MutexAccess)));
    seismograph::record(EventClass::RuntimeTask, || {
        Some(Record::runtime(
            EventTimestamp::now(),
            EventKind::RuntimeCreated,
            RuntimeEvent {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: None,
                subject_id: 0,
                related_id: 0,
                value_0: 0,
                value_1: 0,
            },
            BacktraceCapture::Configured,
        ))
    });

    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = snapshot::decode(encoded.as_bytes()).unwrap();
    assert_eq!(
        decoded
            .events
            .events
            .iter()
            .map(|event| (event.kind, event.call_stack.is_empty()))
            .collect::<Vec<_>>(),
        [
            (
                EventKind::MutexAccess,
                cfg!(any(miri, not(any(target_os = "windows", target_os = "linux", target_os = "macos"))))
            ),
            (EventKind::RuntimeCreated, true),
        ]
    );
}

#[expect(clippy::unwrap_used, reason = "this test source is registered only after recording starts")]
fn capture_observation(context: SnapshotContext<'_>) -> Result<SourceData, seismograph::Error> {
    let observation: snapshot::Observation = context.recording_observation().unwrap();
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&observation.generation.get().to_le_bytes());
    bytes[8..].copy_from_slice(&observation.observed_at.ticks().to_le_bytes());
    SourceData::copy_from(&bytes)
}

#[test]
fn source_observations_keep_the_recording_generation_and_stop_boundary() {
    static SOURCE: Source = Source::new(
        SourceId::new(0x7265_636f_7264_696e),
        "recording observation",
        1,
        capture_observation,
    );
    let _recording = Recording::start(Configuration {
        general_events: Policy::all(false),
        ..Default::default()
    });
    let recording_generation = generation().unwrap();
    snapshot::register_source(&SOURCE);
    let capture = |disposition| {
        let encoded = seismograph::snapshot(SnapshotOptions {
            event_buffers: disposition,
        })
        .unwrap();
        let decoded = snapshot::decode(encoded.as_bytes()).unwrap();
        decoded
            .sources
            .into_iter()
            .find(|source| source.id == SourceId::new(0x7265_636f_7264_696e))
            .unwrap()
            .data
    };

    let stopped = capture(EventBufferDisposition::Stop);
    assert_eq!(u64::from_le_bytes(stopped[..8].try_into().unwrap()), recording_generation.get());
    assert_eq!(capture(EventBufferDisposition::Retain), stopped);
}
