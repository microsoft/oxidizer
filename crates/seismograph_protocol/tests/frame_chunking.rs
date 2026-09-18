// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration coverage for frame payloads that cross a read-chunk boundary.

use seismograph_protocol::message::Response;
use seismograph_protocol::{read_response, write_response};

#[test]
fn large_response_round_trip_covers_the_normal_library_chunking_path() {
    let payload = vec![0xA5; 8 * 1024 + 1];
    let mut encoded = Vec::new();
    write_response(&mut encoded, 42, &Response::Snapshot(payload.clone())).unwrap();

    let (request_id, response) = read_response(&mut encoded.as_slice()).unwrap();

    assert_eq!(request_id, 42);
    assert!(matches!(response, Response::Snapshot(decoded) if decoded == payload));
}
