// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Plain runtime behavior is independent of the opt-in diagnostics integration.

testing_aids::init_tracing!();

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
fn one_feature_enables_runtime_registration_and_performables_events() {
    use arty::runtime::{Runtime, WorkersPolicy};
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
    let _reset = Reset;
    seismograph::recorder(Configuration {
        general_events: RecordingPolicy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(256).unwrap(),
        ..Configuration::default()
    });
    let value = performables::arc::Arc::new(42);
    let clone = performables::arc::Arc::clone(&value);
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let runtime_id = runtime.id();
    assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
    runtime.stop().unwrap();
    let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    let runtime_source = decoded.sources.iter().find(|entry| entry.id == source::ID).unwrap();
    let runtimes = seismograph_runtime::snapshot::decode(&runtime_source.data).unwrap();
    assert!(runtimes.runtimes.iter().any(|entry| entry.id.get() == runtime_id.get()));
    assert!(decoded.events.events.iter().any(|event| event.kind == EventKind::ArcCreate));
    assert!(decoded.events.events.iter().any(|event| event.kind == EventKind::ArcClone));
    drop((clone, value));
}
