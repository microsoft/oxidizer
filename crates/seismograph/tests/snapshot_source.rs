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
