// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time backend selection and allocation ownership.

use std::alloc::{GlobalAlloc, Layout};

use rallocator::{Rallocator, backend};

rallocator::rallocator!(backend = snmalloc);

const _: &Rallocator<backend::Snmalloc> = &GLOBAL;

#[test]
fn constructed_backend_handles_keep_live_allocations_separate() {
    // SAFETY: Each block stays with its creating backend in this linked crate.
    let snmalloc = unsafe { Rallocator::new(backend::Snmalloc) };
    // SAFETY: The mimalloc handle likewise retires only its own allocations.
    let mimalloc = unsafe { Rallocator::new(backend::Mimalloc) };
    let layout = Layout::from_size_align(41, 64).unwrap();

    // SAFETY: The layout is valid and nonzero.
    let left = unsafe { snmalloc.alloc(layout) };
    // SAFETY: The same layout requests an independent live allocation.
    let right = unsafe { mimalloc.alloc(layout) };
    assert!(!left.is_null() && !right.is_null());
    assert_ne!(left, right);
    // SAFETY: Each successful allocation owns all layout bytes.
    unsafe { left.write_bytes(0x35, layout.size()) };
    // SAFETY: The second backend owns a separate writable allocation.
    unsafe { right.write_bytes(0xa6, layout.size()) };
    // SAFETY: Both allocations remain live and were fully initialized.
    let left_payload = unsafe { std::slice::from_raw_parts(left, layout.size()) };
    // SAFETY: The second allocation remains live and fully initialized.
    let right_payload = unsafe { std::slice::from_raw_parts(right, layout.size()) };
    assert_eq!(
        (
            left.addr() % layout.align(),
            right.addr() % layout.align(),
            left_payload,
            right_payload
        ),
        (0, 0, &[0x35; 41][..], &[0xa6; 41][..])
    );

    // SAFETY: Each block returns to its creating backend with its exact layout.
    unsafe { snmalloc.dealloc(left, layout) };
    // SAFETY: The second block is likewise retired once through its own backend.
    unsafe { mimalloc.dealloc(right, layout) };
}
