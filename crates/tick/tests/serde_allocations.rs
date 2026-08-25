// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation contracts for timestamp deserialization.

#![cfg(all(feature = "fmt", feature = "serde", not(miri)))]
#![expect(clippy::unwrap_used, reason = "test code uses unwrap for concise failure backtraces")]

use alloc_tracker::{Allocator, Session};
use serde::Deserialize;
use serde::de::value::{BytesDeserializer, Error as ValueError, StrDeserializer};
use serde::de::{DeserializeOwned, Visitor};
use tick::fmt::{EcmaScript, Iso8601, Rfc2822, UnixSeconds};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn total_bytes_allocated(session: &Session, operation_name: &str) -> u64 {
    session
        .to_report()
        .operations()
        .find_map(|(name, operation)| (name == operation_name).then(|| operation.total_bytes_allocated()))
        .unwrap()
}

fn assert_deserialize_without_allocation<T: DeserializeOwned>(input: &str, operation_name: &str) {
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation(operation_name);
    {
        let _span = operation.measure_thread().iterations(1);
        std::hint::black_box(serde_json::from_str::<T>(input).unwrap());
    }

    assert_eq!(
        total_bytes_allocated(&session, operation_name),
        0,
        "{operation_name} must deserialize borrowed JSON without allocating"
    );
}

#[test]
fn borrowed_json_deserialization_is_allocation_free() {
    assert_deserialize_without_allocation::<Iso8601>(r#""2024-08-06T21:30:00Z""#, "iso_8601");
    assert_deserialize_without_allocation::<Rfc2822>(r#""Tue, 06 Aug 2024 21:30:00 GMT""#, "rfc_2822");
    assert_deserialize_without_allocation::<UnixSeconds>("1722979800", "unix_seconds");
    assert_deserialize_without_allocation::<EcmaScript>(r#""2024-08-06T21:30:00.123Z""#, "ecmascript");
}

#[test]
fn textual_format_rejects_non_string_json_with_clear_expectation() {
    let error = serde_json::from_str::<Iso8601>("123").unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid type: integer `123`, expected a timestamp string at line 1 column 3"
    );
}

#[test]
fn textual_format_deserializes_transient_string() {
    let deserializer = StrDeserializer::<ValueError>::new("2024-08-06T21:30:00Z");
    let iso = Iso8601::deserialize(deserializer).unwrap();

    assert_eq!(iso.to_string(), "2024-08-06T21:30:00Z");
}

#[test]
fn textual_format_deserializes_utf8_bytes() {
    let deserializer = BytesDeserializer::<ValueError>::new(b"2024-08-06T21:30:00Z");
    let iso = Iso8601::deserialize(deserializer).unwrap();

    assert_eq!(iso.to_string(), "2024-08-06T21:30:00Z");
}

#[test]
fn textual_format_rejects_invalid_utf8_bytes() {
    let deserializer = BytesDeserializer::<ValueError>::new(b"\xFF");
    let error = Iso8601::deserialize(deserializer).unwrap_err();

    assert!(error.to_string().contains("invalid utf-8 sequence"));
}

#[test]
fn textual_format_requests_string_deserialization() {
    struct StringOnlyDeserializer;

    impl<'de> serde::Deserializer<'de> for StringOnlyDeserializer {
        type Error = ValueError;

        fn deserialize_any<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
        where
            V: Visitor<'de>,
        {
            Err(serde::de::Error::custom("only string deserialization is supported"))
        }

        fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: Visitor<'de>,
        {
            visitor.visit_borrowed_str("2024-08-06T21:30:00Z")
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str bytes
            byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
            map struct enum identifier ignored_any
        }
    }

    let iso = Iso8601::deserialize(StringOnlyDeserializer).unwrap();

    assert_eq!(iso.to_string(), "2024-08-06T21:30:00Z");
}
