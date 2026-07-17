// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Capture dynamic input in an arena value, replay it through ordinary Serde,
//! and apply resource limits to untrusted input.

#![allow(clippy::missing_panics_doc, reason = "example code")]

use multitude::de::{DeserializationLimits, Value};
use multitude::{Arena, Box};
use serde::Deserialize as _;

#[derive(Debug, PartialEq, serde::Deserialize)]
struct Coordinates {
    x: i64,
    y: i64,
}

fn main() -> serde_json::Result<()> {
    let arena = Arena::new();

    let value: Box<Value> = arena.deserialize_json(r#"{"x":3,"y":4,"label":"point","label":"duplicate"}"#)?;
    assert_eq!(value.get_all("label").count(), 2);

    // Value implements Deserializer by reference, so buffered data can be
    // replayed into an ordinary Serde type without reparsing the source bytes.
    let coordinates = Coordinates::deserialize(&*value).expect("the captured fields match Coordinates");
    assert_eq!(coordinates, Coordinates { x: 3, y: 4 });

    let limits = DeserializationLimits::unlimited()
        .with_max_depth(4)
        .with_max_sequence_len(8)
        .with_max_map_len(4)
        .with_max_string_len(32)
        .with_max_bytes_len(32);
    let sequence_limits = limits.with_max_sequence_len(2);
    let result: serde_json::Result<Box<Value>> = arena.deserialize_json_with_limits(r#"["one","two","three"]"#, sequence_limits);
    let error = result.expect_err("the sequence exceeds the configured limit");
    assert!(error.to_string().contains("sequence length limit"));

    Ok(())
}
