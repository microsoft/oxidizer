// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stable optimized call site for the once-per-cycle demand check.

use seismograph_runtime::worker::{WorkerHandle, WorkerMetadata, WorkerRole};
use seismograph_runtime::{RuntimeMetadata, register_runtime};

#[inline(never)]
fn observation_probe(worker: &WorkerHandle) -> bool {
    worker.is_observed()
}

#[test]
fn disabled_cycle_selection_does_not_create_event_buffers() {
    seismograph::recorder(seismograph::recorder::Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("assembly-probe", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let handle = worker.handle();

    let observed = observation_probe(std::hint::black_box(&handle));
    let encoded = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();

    assert_eq!(
        (observed, snapshot.events.events.len(), snapshot.events.threads.len()),
        (false, 0, 0)
    );
}
