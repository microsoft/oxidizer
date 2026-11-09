// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exercises owned source payloads through the public snapshot API.

use seismograph::snapshot::{self, SnapshotContext, SnapshotOptions, Source, SourceData, SourceId, SourceSnapshot};

static SOURCE: Source = Source::new(SourceId::new(1), "source", 1, capture);

fn capture(_context: SnapshotContext<'_>) -> Result<SourceData, seismograph::Error> {
    SourceData::copy_from(b"captured")
}

#[test]
fn source_payload_survives_capture() {
    snapshot::register_source(&SOURCE);
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = std::thread::spawn(move || snapshot::decode(encoded.as_bytes()).unwrap())
        .join()
        .unwrap();
    assert_eq!(
        decoded.sources,
        vec![SourceSnapshot {
            id: SourceId::new(1),
            name: "source".to_owned(),
            schema_version: 1,
            data: b"captured".to_vec(),
        }]
    );
}

#[test]
#[cfg_attr(miri, ignore = "tests native filesystem output and write errors")]
fn snapshot_file_persistence_uses_the_caller_path() {
    use std::error::Error;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("snapshot.bin");
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();

    encoded.write_file(&path).unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), encoded.as_bytes());

    let missing_parent = directory.path().join("missing").join("snapshot.bin");
    let error = encoded.write_file(&missing_parent).unwrap_err();
    let source = error.source().unwrap().downcast_ref::<std::io::Error>().unwrap();
    assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
    assert_eq!(
        error.to_string(),
        format!("failed to write seismograph snapshot to {}: {source}", missing_parent.display())
    );
}

#[test]
fn invalid_utf8_source_name_is_rejected() {
    // Version 1 uses a 32-byte snapshot header and a 24-byte source header.
    let mut bytes = vec![0; 56];
    bytes[..8].copy_from_slice(b"SEISMOG\0");
    bytes[8..10].copy_from_slice(&1_u16.to_le_bytes());
    bytes[28..32].copy_from_slice(&1_u32.to_le_bytes());
    bytes[32..40].copy_from_slice(&1_u64.to_le_bytes());
    bytes[40..42].copy_from_slice(&1_u16.to_le_bytes());
    bytes[42..44].copy_from_slice(&1_u16.to_le_bytes());
    bytes.push(b'a');
    assert_eq!(snapshot::decode(&bytes).unwrap().sources[0].name, "a");

    *bytes.last_mut().unwrap() = 0xff;

    snapshot::decode(&bytes).unwrap_err();
}
