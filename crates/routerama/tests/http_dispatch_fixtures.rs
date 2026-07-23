// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence and allocation-path checks for the HTTP dispatch benchmarks.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture supports three harnesses")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]

use alloc_tracker::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("../benches/common/http_dispatch_scenarios.rs");

#[test]
fn every_framework_fixture_is_equivalent_and_allocation_tracking_is_live() {
    let fixtures = Fixtures::new_checked();
    let allocated_bytes = fixtures.record_allocation_sweeps();
    assert!(
        allocated_bytes.into_iter().all(|bytes| bytes > 0),
        "each dispatch sweep should exercise the tracked allocator: {allocated_bytes:?}"
    );
}
