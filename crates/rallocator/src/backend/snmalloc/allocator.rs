// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::Layout;

use crate::backend::sealed::Operations;
use crate::backend::snmalloc::{classes, observation, thread};

/// Selects the Rust snmalloc backend for [`Rallocator`](crate::Rallocator).
///
/// This is the default backend. Install it with
/// `rallocator!(backend = snmalloc)` or `rallocator!()`.
#[derive(Clone, Copy, Debug, Default)]
#[doc(hidden)]
pub struct Snmalloc;

impl crate::backend::Backend for Snmalloc {}

impl Operations for Snmalloc {
    #[inline]
    unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        if ZEROED {
            thread::allocate_zeroed(layout)
        } else {
            thread::allocate(layout)
        }
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: *mut u8, _layout: Layout) {
        // SAFETY: The caller transfers a live allocation from this backend.
        unsafe { thread::deallocate(ptr) };
    }

    #[inline]
    unsafe fn reallocate(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller supplies the live allocation and valid new size.
        unsafe { reallocate_core(ptr, layout, new_size) }
    }

    #[inline]
    unsafe fn prepare_reallocation(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Some(request) = classes::Request::with_size(layout, new_size) else {
            return std::ptr::null_mut();
        };
        if classes::Request::new(layout) == Some(request) {
            return ptr;
        }
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        let replacement = thread::allocate_request(request);
        if !replacement.is_null() {
            // SAFETY: Both allocations remain live and disjoint for the copy bound.
            unsafe { std::ptr::copy_nonoverlapping(ptr, replacement, layout.size().min(new_size)) };
        }
        replacement
    }

    fn publish_state(&self) {
        observation::publish();
    }
}

#[inline]
pub(super) unsafe fn reallocate_core(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    let Some(request) = classes::Request::with_size(layout, new_size) else {
        return std::ptr::null_mut();
    };
    // override/rust.cc retains a pointer exactly when the aligned old and
    // new layouts select the same size class, not for arbitrary shrinking.
    if classes::Request::new(layout) == Some(request) {
        return ptr;
    }
    // SAFETY: The caller guarantees ptr is live for layout; the copy bound
    // fits both the old allocation and requested replacement.
    unsafe { thread::reallocate(ptr, request, layout.size().min(new_size)) }
}
