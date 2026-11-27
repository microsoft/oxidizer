// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::{GlobalAlloc, Layout};

use crate::backend::sealed::Operations;

/// Selects [`std::alloc::System`] for [`Rallocator`](crate::Rallocator).
///
/// This is the default backend. Install it with
/// `rallocator!(backend = system)` or `rallocator!()`.
/// [`Rallocator`](crate::Rallocator) records its allocation events as it does
/// for the other backends. This backend publishes no allocator state.
#[derive(Clone, Copy, Debug, Default)]
#[doc(hidden)]
pub struct System;

impl crate::backend::Backend for System {}

// SAFETY: The platform allocator owns every block and upholds GlobalAlloc's contracts.
// Resize preparation allocates disjoint storage without releasing the original,
// as required by the recorded resize protocol.
unsafe impl Operations for System {
    unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        if ZEROED {
            // SAFETY: The caller supplies a valid nonzero allocation layout.
            unsafe { std::alloc::System.alloc_zeroed(layout) }
        } else {
            // SAFETY: The same layout is valid for an uninitialized allocation.
            unsafe { std::alloc::System.alloc(layout) }
        }
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: The caller supplies a live System block and its exact layout.
        unsafe { std::alloc::System.dealloc(ptr, layout) };
    }

    #[inline]
    unsafe fn reallocate(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The live System block, layout and new size meet GlobalAlloc's resize contract.
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }

    fn publish_state(&self) {}
}
