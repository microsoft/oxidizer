// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stable optimized call site for the once-per-cycle demand check.

use seismograph_runtime::asynchronous::Worker;
use seismograph_runtime::{Runtime, WorkerMetadata, WorkerRole};

#[inline(never)]
fn observation_probe(worker: &Worker) -> bool {
    worker.is_observed()
}

#[test]
fn disabled_cycle_selection_does_not_create_event_buffers() {
    seismograph::recorder(seismograph::recorder::Configuration::default());
    let runtime = Runtime::builder("assembly-probe").configured_workers(1).build();
    let worker = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));

    let observed = observation_probe(std::hint::black_box(&worker));
    let encoded = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();

    assert_eq!(
        (observed, snapshot.events.events.len(), snapshot.events.threads.len()),
        (false, 0, 0)
    );
}
