// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public event recording, source observations, and scoped suppression.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, MutexGuard};

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp, ObjectId, Record};
use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId};
use seismograph::recorder::{Configuration, EventBufferCapacity, Policy, is_enabled, without_recording};
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
fn record_rejects_mismatched_event_categories() {
    let _recording = Recording::start(Configuration {
        general_events: Policy::all(false),
        arc_dereferences: Policy::all(false),
        ..Default::default()
    });
    seismograph::record(EventClass::General, || Some(object(EventKind::MutexAccess)));
    seismograph::record(EventClass::ArcDereference, || Some(object(EventKind::ArcDeref)));
    seismograph::record(EventClass::General, || Some(object(EventKind::ArcDeref)));
    seismograph::record(EventClass::ArcDereference, || Some(object(EventKind::MutexRelease)));
    seismograph::record(EventClass::General, || Some(object(EventKind::MutexRelease)));

    let captured = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = snapshot::decode(captured.as_bytes()).unwrap();
    assert_eq!(
        decoded.events.events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        vec![EventKind::MutexAccess, EventKind::ArcDeref, EventKind::MutexRelease],
    );
}

#[test]
fn category_queries_follow_thread_suppression() {
    let _recording = Recording::start(Configuration {
        general_events: Policy::all(false),
        ..Default::default()
    });
    let observe = || (is_enabled(EventClass::General), is_enabled(EventClass::Allocation));

    let before = observe();
    let suppressed = without_recording(observe);
    let after = observe();

    assert_eq!((before, suppressed, after), ((true, false), (false, false), (true, false)));
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))] // Disabled and stopped recording deliberately skips these builders.
fn disabled_and_stopped_recording_skip_builders() {
    let _recording = Recording::start(Configuration::default());
    let constructed = Cell::new(0);
    let build = || {
        constructed.set(constructed.get() + 1);
        Some(object(EventKind::MutexAccess))
    };
    assert!(!is_enabled(EventClass::General));
    seismograph::record(EventClass::General, build);
    seismograph::record(EventClass::General, build);
    assert_eq!(constructed.get(), 0);

    let configuration = Configuration {
        general_events: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64),
        ..Default::default()
    };
    seismograph::recorder(configuration);
    assert!(is_enabled(EventClass::General));
    seismograph::record(EventClass::General, build);
    seismograph::recorder(configuration);

    seismograph::recorder::clear_buffers().unwrap();
    assert_eq!(constructed.get(), 1);
    seismograph::record(EventClass::General, build);
    assert_eq!(constructed.get(), 2);

    let _capture = seismograph::snapshot(SnapshotOptions {
        event_buffers: EventBufferDisposition::Stop,
    })
    .unwrap();
    seismograph::record(EventClass::General, build);
    seismograph::record(EventClass::General, build);
    assert_eq!(constructed.get(), 2);
}

#[test]
#[cfg_attr(coverage_nightly, coverage(off))] // Suppression deliberately prevents the builders from running.
fn suppression_is_nested_thread_local_and_restored_after_unwinding() {
    let _recording = Recording::start(Configuration {
        general_events: Policy::all(false),
        ..Default::default()
    });
    let constructed = Cell::new(false);

    without_recording(|| {
        assert!(!is_enabled(EventClass::General));
        seismograph::record(EventClass::General, || {
            constructed.set(true);
            Some(object(EventKind::MutexAccess))
        });
        seismograph::record(EventClass::General, || {
            constructed.set(true);
            Some(object(EventKind::MutexAccess))
        });
        without_recording(|| assert!(!is_enabled(EventClass::General)));
        assert!(!is_enabled(EventClass::General));
        assert!(std::thread::spawn(|| is_enabled(EventClass::General)).join().unwrap());
    });
    assert!(!constructed.get());
    assert!(is_enabled(EventClass::General));

    let panic = catch_unwind(AssertUnwindSafe(|| {
        without_recording(|| panic!("test suppression unwinding"));
    }));
    assert!(panic.is_err());
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
    assert!(is_enabled(EventClass::General));
    seismograph::record(EventClass::General, || None);
    seismograph::record(EventClass::General, || None);

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

    seismograph::record(EventClass::General, || {
        seismograph::recorder(configuration);
        Some(object(EventKind::MutexAccess))
    });
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

fn capture_events(context: SnapshotContext<'_>) -> Result<SourceData, seismograph::Error> {
    SourceData::copy_from(&context.events().total_events.to_le_bytes())
}

#[test]
fn sources_receive_the_events_captured_before_stop() {
    static SOURCE: Source = Source::new(SourceId::new(0x7265_636f_7264_696e), "recording events", 1, capture_events);
    let _recording = Recording::start(Configuration {
        general_events: Policy::all(false),
        ..Default::default()
    });
    seismograph::record(EventClass::General, || Some(object(EventKind::MutexAccess)));
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
    assert_eq!(u64::from_le_bytes(stopped[..8].try_into().unwrap()), 1);
    assert_eq!(u64::from_le_bytes(capture(EventBufferDisposition::Retain).try_into().unwrap()), 0);
}

#[test]
fn recorder_thread_identity_remains_available_to_late_tls_destructors() {
    use std::cell::RefCell;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct CaptureOnDrop(Arc<AtomicU64>);
    impl Drop for CaptureOnDrop {
        fn drop(&mut self) {
            self.0.store(seismograph::recorder::thread_id().get(), Ordering::Release);
        }
    }
    thread_local! {
        static LATE: RefCell<Option<CaptureOnDrop>> = const { RefCell::new(None) };
    }
    let _recording = Recording::start(Configuration::default());
    let captured = Arc::new(AtomicU64::new(0));
    let result = Arc::clone(&captured);

    let expected = std::thread::spawn(move || {
        LATE.set(Some(CaptureOnDrop(result)));
        seismograph::recorder::thread_id().get()
    })
    .join()
    .unwrap();

    assert_eq!(captured.load(Ordering::Acquire), expected);
}
