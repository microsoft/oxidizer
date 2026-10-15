// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Plain runtime behavior is independent of the opt-in diagnostics integration.

testing_aids::init_tracing!();
#[cfg(all(feature = "rt", feature = "seismograph"))]
static RECORDING_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(all(feature = "rt", not(feature = "seismograph")))]
#[test]
fn plain_runtime_does_not_register_a_seismograph_runtime_source() {
    use arty::runtime::{Runtime, WorkersPolicy};
    use seismograph::snapshot::SnapshotOptions;
    use seismograph_runtime::snapshot::source;

    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
    runtime.stop().unwrap();
    let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    assert!(decoded.sources.iter().all(|entry| entry.id != source::ID));
}

#[cfg(all(feature = "rt", feature = "seismograph"))]
#[test]
#[cfg_attr(
    miri,
    ignore = "native diagnostic endpoint opt-in; instrumentation cases remain replayed in the Arty library under Miri"
)]
fn one_feature_enables_runtime_registration_and_performables_events() {
    use arty::runtime::{MonitorOptions, Runtime, WorkersPolicy};
    use seismograph::recorder::event::EventKind;
    use seismograph::recorder::{Configuration, EventBufferCapacity, RecordingPolicy};
    use seismograph::snapshot::SnapshotOptions;
    use seismograph_runtime::snapshot::source;

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            seismograph::recorder(Configuration::default());
        }
    }
    let _lock = RECORDING_LOCK.lock().unwrap();
    let _reset = Reset;
    seismograph::recorder(Configuration {
        general_events: RecordingPolicy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(256).unwrap(),
        ..Configuration::default()
    });
    let value = performables::arc::Arc::new(42);
    let clone = performables::arc::Arc::clone(&value);
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .monitor(MonitorOptions::default().name("feature-monitor-test"))
        .build()
        .unwrap();
    assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
    runtime.stop().unwrap();
    let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    let runtime_source = decoded.sources.iter().find(|entry| entry.id == source::ID).unwrap();
    let runtimes = seismograph_runtime::snapshot::decode(&runtime_source.data).unwrap();
    assert!(runtimes.runtimes.iter().any(|entry| entry.name == "feature-monitor-test"));
    assert!(decoded.events.events.iter().any(|event| event.kind == EventKind::ArcCreate));
    assert!(decoded.events.events.iter().any(|event| event.kind == EventKind::ArcClone));
    drop((clone, value));
}

#[cfg(all(feature = "rt", feature = "seismograph"))]
#[test]
fn compiled_diagnostics_without_monitor_opt_in_do_not_register_runtime_sources() {
    use arty::runtime::{Runtime, WorkersPolicy};
    use seismograph::recorder::{Configuration, RecordingPolicy};
    use seismograph::snapshot::SnapshotOptions;
    use seismograph_runtime::snapshot::source;

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            seismograph::recorder(Configuration::default());
        }
    }
    let _lock = RECORDING_LOCK.lock().unwrap();
    let _reset = Reset;
    seismograph::recorder(Configuration {
        runtime_tasks: RecordingPolicy::all(false),
        ..Configuration::default()
    });
    seismograph::recorder::clear_event_buffers().unwrap();
    let ids = || {
        let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
        let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
        snapshot
            .sources
            .iter()
            .find(|entry| entry.id == source::ID)
            .map_or_else(Vec::new, |entry| {
                seismograph_runtime::snapshot::decode(&entry.data)
                    .unwrap()
                    .runtimes
                    .into_iter()
                    .map(|runtime| runtime.id)
                    .collect::<Vec<_>>()
            })
    };
    let before = ids();
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
    runtime.stop().unwrap();
    assert_eq!(ids(), before);
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    assert!(!snapshot.events.events.iter().any(|event| event.runtime().is_some()));
}
