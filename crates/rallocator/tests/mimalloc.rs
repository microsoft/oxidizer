// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-process contracts for the Rust mimalloc backend.

use GLOBAL as Allocator;

rallocator::rallocator!(backend = mimalloc);

const _: &rallocator::Rallocator<rallocator::backend::Mimalloc> = &GLOBAL;

#[path = "common/allocator.rs"]
mod contracts;

#[test]
fn unsupported_native_alignment_fails_without_losing_existing_allocations() {
    use std::alloc::{GlobalAlloc, Layout};

    let layout = Layout::from_size_align(37, 16).unwrap();
    // SAFETY: The layout requests valid, nonzero storage.
    let live = unsafe { Allocator.alloc(layout) };
    assert!(!live.is_null());
    // SAFETY: This checked allocation owns every layout byte.
    unsafe { live.write_bytes(0x5a, layout.size()) };
    let unsupported = Layout::from_size_align(1, 256 * 1024 * 1024).unwrap();

    // SAFETY: The layout is valid, but the native aligned-metadata mode does
    // not support alignments at least as large as its 256-MiB metadata region.
    let failure = unsafe { Allocator.alloc(unsupported) };
    assert!(failure.is_null());
    // SAFETY: A failed independent allocation leaves this initialized block live.
    assert_eq!(unsafe { live.read() }, 0x5a);
    // SAFETY: The original allocation is retired once using its exact layout.
    unsafe { Allocator.dealloc(live, layout) };
}
