// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Defaults and skipped fields in both map and sequence representations.

#![cfg(feature = "serde_json")]

use multitude::Arena;
use multitude::de::DeserializeIn;

#[derive(Debug, PartialEq, DeserializeIn)]
#[serde(default)]
struct Record {
    value: u32,
    #[serde(skip)]
    skipped: u32,
    #[serde(default)]
    explicit: u32,
}

impl Default for Record {
    fn default() -> Self {
        Self {
            value: 17,
            skipped: 29,
            explicit: 43,
        }
    }
}

#[derive(Debug, PartialEq, DeserializeIn)]
struct Mixed {
    first: u32,
    #[serde(skip)]
    ignored: u32,
    last: u32,
}

#[derive(Debug, PartialEq, DeserializeIn)]
enum Tuple {
    Values(u32, #[serde(skip)] u32, u32),
}

#[test]
fn gamma_round2_container_defaults_do_not_replace_explicit_values_or_field_defaults() {
    let arena = Arena::new();
    for source in ["{}", "[]"] {
        assert_eq!(
            arena.deserialize_json::<Record, _>(source).unwrap(),
            Record {
                value: 17,
                skipped: 29,
                explicit: 0
            }
        );
    }
    for source in [r#"{"value":71,"skipped":99,"explicit":83}"#, "[71,83]"] {
        assert_eq!(
            arena.deserialize_json::<Record, _>(source).unwrap(),
            Record {
                value: 71,
                skipped: 29,
                explicit: 83
            }
        );
    }
}

#[test]
fn gamma_round2_mixed_skip_sequence_reads_only_visible_fields() {
    let arena = Arena::new();
    for source in [r#"{"first":11,"last":37,"ignored":99}"#, "[11,37]"] {
        assert_eq!(
            arena.deserialize_json::<Mixed, _>(source).unwrap(),
            Mixed {
                first: 11,
                ignored: 0,
                last: 37
            }
        );
    }
    assert_eq!(
        arena.deserialize_json::<Tuple, _>(r#"{"Values":[11,37]}"#).unwrap(),
        Tuple::Values(11, 0, 37)
    );
}
