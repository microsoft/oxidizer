// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Equivalence and allocation contracts for UTF-8 body extractor fixtures.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]

use alloc_tracker::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("../benches/common/text_body_scenarios.rs");

#[test]
fn text_and_utf8_body_boundaries_preserve_values_and_rejections() {
    assert_equivalent();
}

#[test]
fn utf8_body_retains_the_single_transport_frame() {
    assert_utf8_api_retains_single_frame();
}

#[test]
fn extraction_allocations_exclude_preparation_and_pin_zero_copy() {
    let diagnostics = allocation_diagnostics();
    assert_eq!(
        diagnostics.map(|(_, stats)| stats.text.allocations),
        [0, 1, 1, 1, 1, 0, 0],
        "TextBody's established allocation behavior changed"
    );
    assert_eq!(
        diagnostics.map(|(_, stats)| stats.utf8.allocations),
        [0, 0, 1, 0, 0, 0, 0],
        "Utf8Body must retain valid single frames and allocate only to combine split frames"
    );
    assert_eq!(
        diagnostics[2].1.utf8.bytes,
        (SPLIT_FIRST.len() + SPLIT_SECOND.len()) as u64,
        "the split path should allocate exactly the collector's combined buffer"
    );
}
