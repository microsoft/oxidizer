// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation contracts for timestamp deserialization.

#![cfg(all(feature = "fmt", feature = "serde", not(miri)))]

use alloc_tracker::{Allocator, Session};
use serde::de::DeserializeOwned;
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
