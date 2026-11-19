// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public deserialization boundary and replay contracts.

#![cfg(feature = "serde_json")]

use core::marker::PhantomData;

use multitude::de::{DeserializationLimits, DeserializationResource, DeserializeIn, Entry, EnumValue, Number, Value};
use multitude::{Arena, Box};
use serde::Deserialize;
use serde::de::IntoDeserializer;
use serde::de::value::Error as ReplayError;

mod common;

#[test]
fn gamma_survivor_integer_widths_and_all_boundaries() {
    let arena = Arena::new();
    macro_rules! capture {
        ($ty:ty, $variant:ident) => {
            for value in [<$ty>::MIN, 0, 1, <$ty>::MAX] {
                let captured: Result<Value, ReplayError> = Value::deserialize_in(&arena, value.into_deserializer());
                assert_eq!(captured.unwrap().as_number(), Some(&Number::$variant(value)));
                let source: Value = Value::Number(Number::$variant(value));
                assert_eq!(<$ty>::deserialize(&source).unwrap(), value);
                assert_eq!(serde_json::to_string(&source).unwrap(), value.to_string());
            }
        };
    }
    capture!(i8, I8);
    capture!(i16, I16);
    capture!(i32, I32);
    capture!(i64, I64);
    capture!(i128, I128);
    capture!(u8, U8);
    capture!(u16, U16);
    capture!(u32, U32);
    capture!(u64, U64);
    capture!(u128, U128);
    macro_rules! replay_boundaries {
        ($ty:ty) => {
            let min = i128::try_from(<$ty>::MIN).unwrap();
            let max = u128::try_from(<$ty>::MAX).unwrap();
            for value in [
                i128::MIN,
                -1,
                0,
                1,
                min,
                min.saturating_sub(1),
                min.saturating_add(1),
                i128::MAX,
            ] {
                let source: Value = if <$ty>::BITS <= 64 {
                    Value::Number(i64::try_from(value).map_or(Number::I128(value), Number::I64))
                } else {
                    Value::Number(Number::I128(value))
                };
                let actual = <$ty>::deserialize(&source);
                match <$ty>::try_from(value) {
                    Ok(expected) => assert_eq!(actual.unwrap(), expected),
                    Err(_) => gamma_survivor_expect_error(actual),
                }
            }
            for value in [0, 1, max.saturating_sub(1), max, max.saturating_add(1), u128::MAX] {
                let source: Value = if <$ty>::BITS <= 64 {
                    Value::Number(u64::try_from(value).map_or(Number::U128(value), Number::U64))
                } else {
                    Value::Number(Number::U128(value))
                };
                let actual = <$ty>::deserialize(&source);
                match <$ty>::try_from(value) {
                    Ok(expected) => assert_eq!(actual.unwrap(), expected),
                    Err(_) => gamma_survivor_expect_error(actual),
                }
            }
        };
    }
    replay_boundaries!(i8);
    replay_boundaries!(i16);
    replay_boundaries!(i32);
    replay_boundaries!(i64);
    replay_boundaries!(i128);
    replay_boundaries!(u8);
    replay_boundaries!(u16);
    replay_boundaries!(u32);
    replay_boundaries!(u64);
    // u128::MIN fits i128; its MAX is covered without a narrowing cast.
    replay_boundaries!(u128);
    replay_boundaries!(isize);
    replay_boundaries!(usize);
}

#[derive(Debug, Deserialize, PartialEq)]
struct ReplayRecord {
    value: u8,
}

#[derive(Debug, Deserialize, PartialEq)]
enum ReplayEnum {
    Unit,
    New(u8),
    Tuple(u8, u16),
    Record { value: u8 },
}

#[test]
fn gamma_survivor_value_type_errors_are_exact() {
    let value: Value = Value::Bool(true);
    macro_rules! error {
        ($ty:ty, $source:expr, $expected:literal) => {
            assert_eq!(<$ty>::deserialize($source).unwrap_err().to_string(), $expected);
        };
    }
    error!(
        bool,
        &Value::<allocator_api2::alloc::Global>::Unit,
        "invalid type non-boolean, expected a boolean"
    );
    error!(char, &value, "invalid type non-character, expected a character");
    error!(String, &value, "invalid type non-string, expected a string");
    error!((), &value, "invalid type non-unit, expected unit");
    error!(Vec<u8>, &value, "invalid type non-sequence, expected a sequence");
    error!((u8, u8), &value, "invalid type non-sequence, expected a tuple");
    error!(std::collections::BTreeMap<String, u8>, &value, "invalid type non-map, expected a map");
    error!(
        ReplayRecord,
        &value, "invalid type invalid struct representation, expected a map or sequence"
    );
    error!(
        ReplayEnum,
        &value, "invalid type invalid enum representation, expected an externally tagged enum"
    );
    let arena = Arena::new();
    let sequence: Value = Value::Sequence(arena.alloc_slice_fill_iter_box([Value::Number(Number::U8(1))]));
    error!((u8, u8), &sequence, "invalid type sequence of wrong length, expected a tuple");
    let enumeration = |variant: &str, payload| -> Value {
        Value::Enum {
            variant: arena.alloc_str_box(variant),
            value: payload,
        }
    };
    error!(
        ReplayEnum,
        &enumeration("Unit", EnumValue::Newtype(arena.alloc_box(Value::Bool(true)))),
        "invalid type non-unit enum payload, expected a unit variant"
    );
    error!(
        ReplayEnum,
        &enumeration("New", EnumValue::Unit),
        "invalid type invalid enum payload, expected a newtype variant"
    );
    error!(
        ReplayEnum,
        &enumeration("Record", EnumValue::Unit),
        "invalid type invalid enum payload, expected a struct variant"
    );
    error!(
        ReplayEnum,
        &enumeration("Tuple", EnumValue::Unit),
        "invalid type invalid enum payload, expected a tuple variant"
    );
}

#[test]
fn gamma_survivor_value_accessors_maps_and_enum_replay() {
    let arena = Arena::new();
    let values: [Value; 8] = [
        Value::Unit,
        Value::None,
        Value::Bool(true),
        Value::Number(Number::U16(513)),
        Value::Char('é'),
        Value::String(arena.alloc_str_box("text")),
        Value::Bytes(arena.alloc_slice_copy_box([0_u8, 127, 255])),
        Value::Sequence(arena.alloc_slice_fill_iter_box([Value::Bool(false), Value::Bool(true)])),
    ];
    for (index, value) in values.iter().enumerate() {
        assert_eq!(value.is_null(), index <= 1);
        assert_eq!(value.as_bool(), (index == 2).then_some(true));
        assert_eq!(value.as_number(), (index == 3).then_some(&Number::U16(513)));
        assert_eq!(value.as_str(), (index == 5).then_some("text"));
        assert_eq!(value.as_bytes(), (index == 6).then_some(&[0, 127, 255][..]));
        assert_eq!(value.as_sequence().map(<[_]>::len), (index == 7).then_some(2));
        assert!(value.as_map().is_none());
        assert!(value.get("text").is_none());
        assert_eq!(value.get_all("text").count(), 0);
    }
    let entries = arena.alloc_slice_fill_iter_box([
        Entry {
            key: Value::Bool(true),
            value: Value::Number(Number::U8(99)),
        },
        Entry {
            key: Value::String(arena.alloc_str_box("key")),
            value: Value::Number(Number::U8(1)),
        },
        Entry {
            key: Value::String(arena.alloc_str_box("other")),
            value: Value::Number(Number::U8(3)),
        },
        Entry {
            key: Value::String(arena.alloc_str_box("key")),
            value: Value::Number(Number::U8(2)),
        },
    ]);
    let map: Value = Value::Map(entries);
    assert_eq!(map.as_map().unwrap().len(), 4);
    assert_eq!(map.get("key").unwrap().as_number(), Some(&Number::U8(1)));
    assert_eq!(
        map.get_all("key").map(|value| value.as_number().unwrap()).collect::<Vec<_>>(),
        [&Number::U8(1), &Number::U8(2)]
    );
    assert!(map.get("missing").is_none());
    assert_eq!(map.get_all("missing").count(), 0);

    let cases = [
        ("Unit", EnumValue::Unit, ReplayEnum::Unit, "\"Unit\""),
        (
            "New",
            EnumValue::Newtype(arena.alloc_box(Value::Number(Number::U8(7)))),
            ReplayEnum::New(7),
            "{\"New\":7}",
        ),
        (
            "Tuple",
            EnumValue::Tuple(arena.alloc_slice_fill_iter_box([Value::Number(Number::U8(7)), Value::Number(Number::U16(513))])),
            ReplayEnum::Tuple(7, 513),
            "{\"Tuple\":[7,513]}",
        ),
        (
            "Record",
            EnumValue::Struct(arena.alloc_slice_fill_iter_box([Entry {
                key: Value::String(arena.alloc_str_box("value")),
                value: Value::Number(Number::U8(9)),
            }])),
            ReplayEnum::Record { value: 9 },
            "{\"Record\":{\"value\":9}}",
        ),
    ];
    for (variant, payload, expected, json) in cases {
        let value: Value = Value::Enum {
            variant: arena.alloc_str_box(variant),
            value: payload,
        };
        assert_eq!(ReplayEnum::deserialize(&value).unwrap(), expected);
        assert_eq!(serde_json::to_string(&value).unwrap(), json);
        let captured: Value = arena.deserialize_json(json).unwrap();
        assert_eq!(ReplayEnum::deserialize(&captured).unwrap(), expected);
    }
}

#[derive(Debug, DeserializeIn, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GammaRecord {
    first_value: u8,
    #[serde(alias = "old")]
    last_value: u16,
    #[serde(skip)]
    hidden: u8,
    #[serde(default)]
    optional: u8,
}

#[derive(Debug, DeserializeIn, PartialEq)]
#[serde(rename_all = "kebab-case", rename_all_fields = "SCREAMING_SNAKE_CASE")]
enum GammaEnum {
    Ready,
    #[serde(skip)]
    Hidden,
    #[serde(alias = "legacy")]
    HasValue {
        field_name: u16,
    },
    Pair(u8, u16),
}

#[derive(Debug, DeserializeIn, PartialEq)]
struct GammaCollision<'__arena, '__arena_, __A, __A_, __Serde, const N: usize> {
    first: __A,
    second: __A_,
    third: __Serde,
    marker: PhantomData<(&'__arena (), &'__arena_ (), [u8; N])>,
}

#[test]
fn gamma_survivor_derive_runtime_rename_alias_skip_and_collisions() {
    let arena = Arena::new();
    let record: GammaRecord = arena.deserialize_json(r#"{"firstValue":1,"old":513}"#).unwrap();
    assert_eq!(
        record,
        GammaRecord {
            first_value: 1,
            last_value: 513,
            hidden: 0,
            optional: 0
        }
    );
    let record: GammaRecord = arena.deserialize_json("[1,513,7]").unwrap();
    assert_eq!(
        record,
        GammaRecord {
            first_value: 1,
            last_value: 513,
            hidden: 0,
            optional: 7
        }
    );
    let duplicate = arena
        .deserialize_json::<GammaRecord, _>(r#"{"firstValue":1,"lastValue":2,"old":3}"#)
        .unwrap_err();
    assert_eq!(duplicate.to_string(), "duplicate field `lastValue` at line 1 column 35");
    let missing = arena.deserialize_json::<GammaRecord, _>(r#"{"lastValue":2}"#).unwrap_err();
    assert_eq!(missing.to_string(), "missing field `firstValue` at line 1 column 15");
    for (json, expected) in [
        ("\"ready\"", GammaEnum::Ready),
        (r#"{"has-value":{"FIELD_NAME":513}}"#, GammaEnum::HasValue { field_name: 513 }),
        (r#"{"legacy":{"FIELD_NAME":513}}"#, GammaEnum::HasValue { field_name: 513 }),
        (r#"{"pair":[3,513]}"#, GammaEnum::Pair(3, 513)),
    ] {
        assert_eq!(arena.deserialize_json::<GammaEnum, _>(json).unwrap(), expected);
    }
    let collision: GammaCollision<'_, '_, u8, u16, u32, 2> = arena
        .deserialize_json(r#"{"first":1,"second":513,"third":70000,"marker":null}"#)
        .unwrap();
    assert_eq!(collision.first, 1);
    assert_eq!(collision.second, 513);
    assert_eq!(collision.third, 70000);
    assert_ne!(GammaEnum::Hidden, GammaEnum::Ready);
    gamma_survivor_expect_error(arena.deserialize_json::<GammaEnum, _>("\"hidden\""));
}

#[test]
fn gamma_survivor_limits_boundaries_and_error_sources() {
    let arena = Arena::new();
    let cases = [
        (
            "[1,2]",
            DeserializationLimits::unlimited().with_max_sequence_len(1),
            DeserializationResource::SequenceLength,
            "sequence length",
        ),
        (
            r#"{"a":1,"b":2}"#,
            DeserializationLimits::unlimited().with_max_map_len(1),
            DeserializationResource::MapLength,
            "map length",
        ),
        (
            r#""é""#,
            DeserializationLimits::unlimited().with_max_string_len(1),
            DeserializationResource::StringLength,
            "string length",
        ),
        (
            "[[1]]",
            DeserializationLimits::unlimited().with_max_depth(1),
            DeserializationResource::Depth,
            "nesting depth",
        ),
    ];
    for (input, limits, resource, label) in cases {
        let error = arena.deserialize_json_with_limits::<Value, _>(input, limits).unwrap_err();
        assert!(error.is_limit_exceeded());
        let exceeded = error.limit_exceeded().unwrap();
        assert_eq!(exceeded.resource(), resource);
        assert_eq!(exceeded.limit(), 1);
        assert!(
            error
                .to_string()
                .starts_with(&format!("JSON deserialization exceeded the {label} limit of 1"))
        );
        let source = core::error::Error::source(&error).unwrap();
        assert_eq!(source.to_string(), error.as_json_error().to_string());
        assert!(error.to_string().ends_with(&format!("\n\nCaused by:\n{source}")));
        gamma_survivor_expect_success(arena.deserialize_json_with_limits::<Value, _>(input, DeserializationLimits::unlimited()));
    }
    for (input, limits) in [
        ("[1]", DeserializationLimits::unlimited().with_max_sequence_len(1)),
        ("[]", DeserializationLimits::unlimited().with_max_sequence_len(0)),
        (r#"{"a":1}"#, DeserializationLimits::unlimited().with_max_map_len(1)),
        ("{}", DeserializationLimits::unlimited().with_max_map_len(0)),
        (r#""é""#, DeserializationLimits::unlimited().with_max_string_len(2)),
        (r#""""#, DeserializationLimits::unlimited().with_max_string_len(0)),
    ] {
        gamma_survivor_expect_success(arena.deserialize_json_with_limits::<Value, _>(input, limits));
    }
    let malformed = arena
        .deserialize_json_with_limits::<Value, _>("[", DeserializationLimits::unlimited())
        .unwrap_err();
    assert!(!malformed.is_limit_exceeded());
    assert!(malformed.limit_exceeded().is_none());
    assert!(malformed.to_string().starts_with("JSON deserialization failed"));
    for input in ["true false", "[1] trailing"] {
        gamma_survivor_expect_error(arena.deserialize_json::<Value, _>(input));
        gamma_survivor_expect_error(arena.deserialize_json_with_limits::<Value, _>(input, DeserializationLimits::unlimited()));
    }
}

#[test]
fn gamma_survivor_deserialization_failure_is_returned() {
    let arena = Arena::new_in(common::FailingAllocator::new(0));
    for input in [r#""owned""#, "[1]", r#"{"key":true}"#] {
        gamma_survivor_expect_error(arena.deserialize_json::<Value<_>, _>(input));
    }
    gamma_survivor_expect_error(arena.deserialize_json::<Box<str, _>, _>(r#""owned""#));
    gamma_survivor_expect_error(arena.deserialize_json::<Box<[u8], _>, _>("[1]"));
    gamma_survivor_expect_error(arena.deserialize_json_alloc_with_limits::<u8, _>("1", DeserializationLimits::unlimited()));
}

#[test]
fn gamma_survivor_inner_deserialization_errors_propagate_through_root_owners() {
    use serde::de::value::BorrowedStrDeserializer;

    let arena = Arena::new();
    let input = || BorrowedStrDeserializer::<ReplayError>::new("not a number");
    let expected = "invalid type: string \"not a number\", expected u8";
    assert_eq!(
        multitude::Rc::<u8>::deserialize_in(&arena, input()).unwrap_err().to_string(),
        expected
    );
    assert_eq!(arena.deserialize_alloc::<u8, _>(input()).unwrap_err().to_string(), expected);
    assert_eq!(
        arena
            .deserialize_alloc_with_limits::<u8, _>(input(), DeserializationLimits::unlimited())
            .unwrap_err()
            .to_string(),
        expected
    );
    gamma_survivor_expect_error(arena.deserialize_json::<multitude::Arc<[u8]>, _>(r#"["bad"]"#));
    gamma_survivor_expect_error(arena.deserialize_json::<multitude::Box<[u8]>, _>(r#"["bad"]"#));
}

struct GammaBytes;

impl<'de> serde::de::Visitor<'de> for GammaBytes {
    type Value = Vec<u8>;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("bytes")
    }

    fn visit_borrowed_bytes<E: serde::de::Error>(self, v: &'de [u8]) -> Result<Self::Value, E> {
        Ok(v.to_vec())
    }
}

#[test]
fn gamma_survivor_bytes_and_ordinal_identifiers() {
    let arena = Arena::new();
    let wrong: Value = Value::Bool(true);
    assert_eq!(
        serde::Deserializer::deserialize_bytes(&wrong, GammaBytes).unwrap_err().to_string(),
        "invalid type non-bytes, expected bytes",
    );
    let bytes: Value = Value::Bytes(arena.alloc_slice_copy_box([0_u8, 128, 255]));
    assert_eq!(serde::Deserializer::deserialize_bytes(&bytes, GammaBytes).unwrap(), [0, 128, 255]);
    for bound in [0, 1, 2, 3, usize::MAX] {
        let input = serde::de::value::BorrowedBytesDeserializer::<ReplayError>::new(&[0, 128, 255]);
        let result: Result<Value, _> = arena.deserialize_with_limits(input, DeserializationLimits::unlimited().with_max_bytes_len(bound));
        if bound >= 3 {
            assert_eq!(result.unwrap().as_bytes(), Some(&[0, 128, 255][..]));
        } else {
            gamma_survivor_expect_error(result);
        }
    }
    for first_key in [
        Value::Bytes(arena.alloc_slice_copy_box(*b"firstValue")),
        Value::Number(Number::U64(0)),
    ] {
        let map: Value = Value::Map(arena.alloc_slice_fill_iter_box([
            Entry {
                key: first_key,
                value: Value::Number(Number::U8(1)),
            },
            Entry {
                key: Value::Number(Number::U64(1)),
                value: Value::Number(Number::U16(513)),
            },
        ]));
        let record = GammaRecord::deserialize_in(&arena, &map).unwrap();
        assert_eq!(record.first_value, 1);
        assert_eq!(record.last_value, 513);
        assert_eq!(record.optional, 0);
    }
    for key in [3, u64::MAX] {
        let map: Value = Value::Map(arena.alloc_slice_fill_iter_box([Entry {
            key: Value::Number(Number::U64(key)),
            value: Value::Unit,
        }]));
        gamma_survivor_expect_error(GammaRecord::deserialize_in(&arena, &map));
    }
    let payload: Value = Value::Map(arena.alloc_slice_fill_iter_box([Entry {
        key: Value::Bytes(arena.alloc_slice_copy_box(*b"FIELD_NAME")),
        value: Value::Number(Number::U16(513)),
    }]));
    let enumeration: Value = Value::Map(arena.alloc_slice_fill_iter_box([Entry {
        key: Value::Number(Number::U64(1)),
        value: payload,
    }]));
    assert_eq!(
        GammaEnum::deserialize_in(&arena, &enumeration).unwrap(),
        GammaEnum::HasValue { field_name: 513 }
    );
    assert_eq!(
        arena
            .deserialize_json_with_limits::<GammaEnum, _>(
                r#"{"pair":[3,513]}"#,
                DeserializationLimits::unlimited().with_max_sequence_len(2)
            )
            .unwrap(),
        GammaEnum::Pair(3, 513),
    );
}

#[test]
fn gamma_survivor_json_each_prefix_and_complete_input() {
    use multitude::de::JsonEachError;

    let arena = Arena::new();
    let mut prefix = Vec::new();
    let failure = arena
        .try_deserialize_json_each("[1,2,broken suffix]", |value: u8| {
            prefix.push(value);
            if value == 2 { Err("callback marker") } else { Ok(()) }
        })
        .unwrap_err();
    assert!(matches!(failure, JsonEachError::Callback("callback marker")));
    assert_eq!(prefix, [1, 2]);
    let mut prefix = Vec::new();
    gamma_survivor_expect_error(arena.deserialize_json_each("[1,2] trailing", |value: u8| prefix.push(value)));
    assert_eq!(prefix, [1, 2]);
    let mut prefix = Vec::new();
    let failure = arena
        .deserialize_json_each_with_limits(
            "[1,2,3]",
            DeserializationLimits::unlimited().with_max_sequence_len(2),
            |value: u8| prefix.push(value),
        )
        .unwrap_err();
    assert_eq!(prefix, [1, 2]);
    assert_eq!(
        failure.limit_exceeded().unwrap().resource(),
        DeserializationResource::SequenceLength
    );
    assert_eq!(failure.limit_exceeded().unwrap().limit(), 2);
    gamma_survivor_expect_error(arena.deserialize_json_alloc::<u8, _>("false"));
    let mut reused = arena.alloc_vec_with_capacity::<u8>(4);
    reused.push(99);
    gamma_survivor_expect_error(reused.deserialize_json_reusing("[1,false]"));
    assert_eq!(reused.as_slice(), &[1]);
    assert_eq!(reused.capacity(), 4);
}

#[derive(Debug, DeserializeIn, PartialEq)]
#[serde(default = "gamma_survivor_make_default")]
struct GammaDefaultRecord {
    first: u8,
    #[serde(skip)]
    hidden: u16,
    #[serde(default)]
    last: u8,
}

fn gamma_survivor_make_default() -> GammaDefaultRecord {
    GammaDefaultRecord {
        first: 11,
        hidden: 513,
        last: 99,
    }
}

#[test]
fn gamma_survivor_container_defaults_map_and_sequence() {
    let arena = Arena::new();
    for input in ["{}", "[]"] {
        assert_eq!(
            arena.deserialize_json::<GammaDefaultRecord, _>(input).unwrap(),
            GammaDefaultRecord {
                first: 11,
                hidden: 513,
                last: 0
            },
        );
    }
    for input in [r#"{"first":7,"last":5}"#, "[7,5]"] {
        assert_eq!(
            arena.deserialize_json::<GammaDefaultRecord, _>(input).unwrap(),
            GammaDefaultRecord {
                first: 7,
                hidden: 513,
                last: 5
            },
        );
    }
    for input in [r#"{"first":7}"#, "[7]"] {
        assert_eq!(
            arena.deserialize_json::<GammaDefaultRecord, _>(input).unwrap(),
            GammaDefaultRecord {
                first: 7,
                hidden: 513,
                last: 0
            },
        );
    }
}

#[cfg(test)]
fn gamma_survivor_expect_error<T, E>(result: Result<T, E>) {
    let Err(_) = result else {
        panic!("the fixture requires this operation to return its injected error");
    };
    drop(result);
}

#[cfg(test)]
fn gamma_survivor_expect_success<T, E>(result: Result<T, E>) {
    let Ok(_) = result else {
        panic!("the fixture requires this operation to succeed");
    };
    drop(result);
}
