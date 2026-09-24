// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behavior when rallocator is linked but not installed globally.

use allocation_hints::heaps::{Heap, thread_heap};
use allocation_hints::with_hint;

#[test]
fn passive_hints_remain_usable_with_another_global_allocator() {
    let heap = Heap::new();
    let thread_heap = thread_heap();
    let value = with_hint(&heap, || Box::new(42));
    let thread_value = with_hint(&thread_heap, || Box::new(24));

    assert_eq!(*value, 42);
    assert_eq!(*thread_value, 24);
}
