// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exercises arena-backed source capture through the linked library.
#![expect(clippy::unwrap_used, reason = "The source callback asserts test fixture preconditions")]

use std::alloc::Layout;

use seismograph::snapshot::{
    self, SnapshotContext, SnapshotOptions, Source, SourceData, SourceId, SourceSnapshot, snapshot_arena_allocate,
    snapshot_arena_deallocate, snapshot_collection_active,
};

static SOURCE: Source = Source::new(SourceId::new(1), "arena", 1, capture);

fn capture(_context: SnapshotContext<'_>) -> Result<SourceData, seismograph::Error> {
    assert!(snapshot_collection_active());
    let address = snapshot_arena_allocate(Layout::new::<u64>()).unwrap();
    assert!(!address.is_null());
    assert!(snapshot_arena_deallocate(address));
    SourceData::copy_from(b"captured")
}

#[test]
fn source_payload_survives_arena_collection() {
    snapshot::register_source(&SOURCE);
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = snapshot::decode(encoded.as_bytes()).unwrap();
    assert_eq!(
        (decoded.sources, snapshot_collection_active()),
        (
            vec![SourceSnapshot {
                id: SourceId::new(1),
                name: "arena".to_owned(),
                schema_version: 1,
                data: b"captured".to_vec(),
            }],
            false,
        )
    );
}
