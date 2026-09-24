// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Checks medium refill counters without routing the test harness through Rallocator.

use std::alloc::{GlobalAlloc, Layout};
use std::ptr::NonNull;

mod support;

#[test]
fn fresh_medium_refill_counts_every_committed_span() {
    // SAFETY: this process uses only the standard Rallocator configuration.
    let allocator: rallocator::Rallocator = unsafe { rallocator::Rallocator::new() };
    let layout = Layout::from_size_align(64 * 1024, 16).unwrap();
    // SAFETY: the layout is nonzero and valid.
    let first = NonNull::new(unsafe { allocator.alloc(layout) }).unwrap();
    let before = support::stats().unwrap();
    // SAFETY: the layout is nonzero and valid; the first span remains live.
    let second = NonNull::new(unsafe { allocator.alloc(layout) }).unwrap();
    let refilled = support::stats().unwrap();
    // SAFETY: the layout is nonzero and valid; this consumes the prefetched span.
    let third = NonNull::new(unsafe { allocator.alloc(layout) }).unwrap();
    let cached = support::stats().unwrap();
    for address in [first, second, third] {
        // SAFETY: these are distinct live allocations with their original layout.
        unsafe { allocator.dealloc(address.as_ptr(), layout) };
    }

    assert_eq!(
        (
            refilled.mapped_bytes - before.mapped_bytes,
            refilled.os_mappings - before.os_mappings,
            cached.mapped_bytes - refilled.mapped_bytes,
            cached.os_mappings - refilled.os_mappings,
        ),
        (2 * layout.size(), 1, 0, 0)
    );
}
