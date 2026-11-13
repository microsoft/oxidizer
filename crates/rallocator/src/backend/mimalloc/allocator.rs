// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::Layout;

use crate::backend::mimalloc::{heap, observation};
use crate::backend::sealed::Operations;

/// Selects the Rust mimalloc backend for [`Rallocator`](crate::Rallocator).
///
/// Install it with `rallocator!(backend = mimalloc)`.
/// This backend is experimental. Seismograph snapshots expose mimalloc-specific
/// page and arena observations rather than snmalloc's state model.
/// It implements the default heap without native statistics, profiling, or guarded sampling.
///
/// The non-hardened metadata layout supports alignments below 256 MiB.
/// Larger alignments return allocation failure, as in the native configuration.
/// For platform and deployment requirements, see [`rallocator!`](crate::rallocator).
#[derive(Clone, Copy, Debug, Default)]
#[doc(hidden)]
pub struct Mimalloc;

impl crate::backend::Backend for Mimalloc {}

impl Operations for Mimalloc {
    #[inline]
    unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        heap::allocate::<ZEROED>(layout)
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: *mut u8, _layout: Layout) {
        // SAFETY: The caller transfers a live allocation from this backend.
        unsafe { heap::deallocate(ptr) };
    }

    #[inline]
    unsafe fn reallocate(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller supplies a live allocation and a valid resize request.
        let replacement = unsafe { prepare_reallocation(ptr, layout, new_size) };
        if !replacement.is_null() && replacement != ptr {
            // SAFETY: Preparation copied the payload and left this old block live.
            unsafe { heap::deallocate(ptr) };
        }
        replacement
    }

    #[inline]
    unsafe fn prepare_reallocation(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller forwards a live block and a valid resize request.
        unsafe { prepare_reallocation(ptr, layout, new_size) }
    }

    fn publish_state(&self) {
        observation::publish();
    }
}

/// Prepare the replacement without retiring the old allocation.
///
/// An invalid replacement layout returns null without changing the old block.
///
/// # Safety
/// The pointer belongs to this backend and is live for layout. The new size is
/// nonzero. A successful move leaves both blocks live.
pub(super) unsafe fn prepare_reallocation(pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
        return std::ptr::null_mut();
    };
    std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
    // SAFETY: The caller's live block pins its page geometry.
    let usable = unsafe { heap::usable_size(pointer) };
    // Word-aligned realloc uses alloc.c's floor-half threshold. Larger
    // alignments use alloc-aligned.c's ceil-half threshold.
    let minimum = if layout.align() <= 8 { usable / 2 } else { usable.div_ceil(2) };
    if new_size <= usable && new_size >= minimum {
        return pointer;
    }
    let replacement = if layout.align() <= 8 {
        heap::allocate_regular(new_size)
    } else {
        heap::allocate::<false>(new_layout)
    };
    if !replacement.is_null() {
        // SAFETY: Both allocations remain live, disjoint and cover the copy bound.
        unsafe { std::ptr::copy_nonoverlapping(pointer, replacement, layout.size().min(new_size)) };
    }
    replacement
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_internal_resize_layout_preserves_the_original_allocation() {
        let layout = Layout::new::<[u8; 32]>();
        let pointer = heap::allocate::<false>(layout);
        assert!(!pointer.is_null());
        // SAFETY: The checked allocation owns every byte of the layout.
        unsafe { pointer.write_bytes(0x5b, layout.size()) };

        // SAFETY: This internal helper validates the replacement layout itself.
        // Its live-block and nonzero-size preconditions still hold.
        let failure = unsafe { prepare_reallocation(pointer, layout, usize::MAX) };

        assert!(failure.is_null());
        // SAFETY: A rejected replacement leaves the initialized allocation live.
        assert_eq!(unsafe { std::slice::from_raw_parts(pointer, layout.size()) }, &[0x5b; 32]);
        // SAFETY: This test retires its live allocation exactly once.
        unsafe { heap::deallocate(pointer) };
    }
}
