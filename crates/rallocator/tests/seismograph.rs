// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration between rallocator and the process-wide seismograph snapshot.

rallocator::rallocator!();

#[test]
fn snapshot_contains_rallocator_source() {
    let value = Box::new(42_u64);
    std::hint::black_box(&value);

    let encoded = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();
    let source = snapshot
        .sources
        .iter()
        .find(|source| source.id == seismograph_rallocator::source::ID)
        .unwrap();
    let allocator = seismograph_rallocator::decode(&source.data).unwrap();

    assert_ne!(allocator.stats.allocations, 0);
}
