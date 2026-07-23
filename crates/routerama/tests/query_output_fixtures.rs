// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Output, allocation, and capacity contracts for query-output benchmarks.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]

use alloc_tracker::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("../benches/common/query_scenarios.rs");

#[test]
fn owned_and_streamed_query_outputs_are_equivalent() {
    assert_output_shapes();
}

#[test]
fn allocating_query_output_meets_allocation_and_capacity_gates() {
    for diagnostic in output_diagnostics() {
        assert_eq!(diagnostic.length, diagnostic.shape.expected().len());
        assert!(diagnostic.capacity >= diagnostic.length);
        if diagnostic.shape.expected().is_empty() {
            assert_eq!(diagnostic.allocations, 0);
            assert_eq!(diagnostic.bytes, 0);
            assert_eq!(diagnostic.capacity, 0);
        } else {
            assert_eq!(diagnostic.allocations, 1);
            assert!(diagnostic.bytes >= diagnostic.capacity as u64);
            assert!(diagnostic.capacity <= diagnostic.length.saturating_mul(2));
        }
    }
}

#[test]
fn caller_reserved_query_output_is_allocation_free() {
    let diagnostic = reserved_output_diagnostic();
    assert_eq!(diagnostic.allocations, 0);
    assert_eq!(diagnostic.bytes, 0);
    assert_eq!(diagnostic.length, COMMON.len());
    assert_eq!(diagnostic.capacity, COMMON.len());
}
