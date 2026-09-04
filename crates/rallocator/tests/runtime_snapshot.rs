// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Regression coverage for process-lifetime runtime symbol caches.

#![cfg(not(miri))]

rallocator::rallocator!();

use seismograph::recorder::runtime::TypeDescriptorId;
use seismograph::recorder::{Configuration, RecordingPolicy};
use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};
use seismograph_runtime::{RuntimeMetadata, register_runtime};

#[test]
fn repeated_runtime_snapshots_keep_symbol_cache_alive() {
    seismograph::recorder(Configuration {
        runtime_tasks: RecordingPolicy::all(true),
        ..Configuration::default()
    });
    let runtime = register_runtime(RuntimeMetadata::new("snapshot-cache", 1));
    let _task = runtime.handle().task_spawned(TypeDescriptorId::from_raw(1).unwrap(), None);

    for _ in 0..3 {
        let encoded = seismograph::snapshot(SnapshotOptions {
            event_buffers: EventBufferDisposition::Clear,
        })
        .unwrap();
        let decoded = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();
        let source = decoded
            .sources
            .iter()
            .find(|source| source.id == seismograph_runtime::snapshot::source::ID)
            .unwrap();
        let runtime = seismograph_runtime::snapshot::decode(&source.data).unwrap();
        assert!(!runtime.addresses.is_empty());
    }

    seismograph::recorder(Configuration::default());
}
