// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process allocator forwarding contracts.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use metabench::__private::Allocator;

#[derive(Default)]
struct RejectResize(AtomicUsize);

// SAFETY: Allocation and deallocation use System; failed resizing preserves the original allocation.
unsafe impl GlobalAlloc for RejectResize {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: The caller supplies a live System allocation and its original layout.
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, _: *mut u8, _: Layout, new_size: usize) -> *mut u8 {
        self.0.store(new_size, Ordering::Relaxed);
        std::ptr::null_mut()
    }
}

struct Borrowed<'a>(&'a RejectResize);

// SAFETY: This adapter forwards the unchanged allocation contract to its borrowed allocator.
unsafe impl GlobalAlloc for Borrowed<'_> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller supplies a valid layout.
        unsafe { self.0.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: The caller supplies a live allocation with its original layout.
        unsafe { self.0.dealloc(ptr, layout) };
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller supplies a live allocation, original layout, and valid new size.
        unsafe { self.0.realloc(ptr, layout, new_size) }
    }
}

#[test]
fn gamma_survivor_allocator_forwards_exact_resize_size() {
    let inner = RejectResize::default();
    let allocator = Allocator::new(Borrowed(&inner));
    let layout = Layout::from_size_align(16, 8).unwrap();
    // SAFETY: The layout has valid alignment and nonzero size.
    let pointer = unsafe { allocator.alloc(layout) };
    assert!(!pointer.is_null());
    for new_size in [17, 32, 63] {
        // SAFETY: Failed resizing leaves this allocation live with its original layout.
        assert!(unsafe { allocator.realloc(pointer, layout, new_size) }.is_null());
        assert_eq!(inner.0.load(Ordering::Relaxed), new_size);
    }
    // SAFETY: Every resize failed, so the original layout still matches the allocation.
    unsafe { allocator.dealloc(pointer, layout) };
}
