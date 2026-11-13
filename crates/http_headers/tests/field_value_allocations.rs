// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation-free round trips through retained HTTP header storage.

#![cfg(all(feature = "http", not(miri)))]

use alloc_tracker::{Allocator, Session};
use http::HeaderValue;
use http_headers::FieldValue;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[test]
fn retained_http_values_return_their_owner_without_allocating_a_wrapper() {
    for sensitive in [false, true] {
        let mut original = HeaderValue::from_bytes(&[b'x'; 128]).unwrap();
        original.set_sensitive(sensitive);
        let pointer = original.as_bytes().as_ptr();
        for value in [FieldValue::from(&original), FieldValue::from(original)] {
            let session = Session::new().no_stdout().no_file();
            let operation = session.operation("http_round_trip");
            let converted;
            {
                let _span = operation.measure_thread().iterations(1);
                converted = HeaderValue::try_from(std::hint::black_box(value)).unwrap();
                std::hint::black_box(&converted);
            }
            assert_eq!(converted.as_bytes(), [b'x'; 128]);
            assert_eq!(converted.as_bytes().as_ptr(), pointer);
            assert_eq!(converted.is_sensitive(), sensitive);
            let allocated = session
                .to_report()
                .operations()
                .find_map(|(name, operation)| (name == "http_round_trip").then(|| operation.total_bytes_allocated()))
                .unwrap();
            assert_eq!(allocated, 0);
        }
    }
}
