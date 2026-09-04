// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for allocation hints without explicit initialization.

use allocation_hints::heaps::Heap;

rallocator::rallocator!();

#[test]
fn heap_hints_need_no_explicit_initialization() {
    let heap = Heap::new();

    assert_ne!(heap.id().get(), 0);
}
