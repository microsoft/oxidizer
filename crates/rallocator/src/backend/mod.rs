// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Backend selectors for [`Rallocator`](crate::Rallocator).
//!
//! Internal support for [`rallocator!`](crate::rallocator).
//! The selectors do not implement [`GlobalAlloc`](std::alloc::GlobalAlloc).

pub(crate) mod abort;
mod dynamic;
#[cfg(target_os = "macos")]
mod macos_mutex;
mod mimalloc;
pub(crate) mod snmalloc;

pub use dynamic::Dynamic;
pub use mimalloc::Mimalloc;
pub use snmalloc::Snmalloc;

/// A built-in backend for [`Rallocator`](crate::Rallocator).
///
/// This trait is sealed. Only [`Snmalloc`], [`Mimalloc`], and [`Dynamic`] implement it.
#[doc(hidden)]
pub trait Backend: sealed::Operations {}

pub(crate) mod sealed {
    use std::alloc::Layout;

    /// Native allocation operations trusted by the recording facade.
    ///
    /// # Safety
    ///
    /// `allocate`, `deallocate`, and `reallocate` must uphold the corresponding
    /// [`GlobalAlloc`](std::alloc::GlobalAlloc) alignment, initialization,
    /// resizing, and ownership contracts.
    /// Successful `ZEROED` allocations must initialize every requested byte to zero.
    /// Allocation or resize failure must leave any live input block unchanged.
    /// Successful resizes must preserve the first `layout.size().min(new_size)` bytes.
    /// Preparing a distinct replacement must keep the original live until
    /// deallocation and return disjoint storage.
    /// Operations must support ownership transfer between threads where the
    /// backend's thread-safety guarantees allow it. Methods must not unwind.
    pub unsafe trait Operations {
        /// # Safety
        /// The layout is valid and nonzero.
        unsafe fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8;

        /// # Safety
        /// The pointer is a live allocation from this backend for this exact
        /// layout, and the caller retires it exactly once.
        unsafe fn deallocate(&self, ptr: *mut u8, layout: Layout);

        /// # Safety
        /// The pointer is live for this exact layout and backend. The new size
        /// is nonzero and preserves `GlobalAlloc`'s layout requirements.
        unsafe fn reallocate(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8;

        /// Prepares and copies a resize without retiring the old block.
        ///
        /// Both old and replacement storage stay live on success. The default
        /// moves the block when its size changes. A backend can resize in place.
        ///
        /// # Safety
        /// The pointer and requested size satisfy the same resize contract.
        unsafe fn prepare_reallocation(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            if new_size == layout.size() {
                return ptr;
            }
            let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
                return std::ptr::null_mut();
            };
            // SAFETY: The new layout is valid and the caller supplies a nonzero size.
            let replacement = unsafe { self.allocate::<false>(new_layout) };
            if !replacement.is_null() {
                // SAFETY: Both blocks are live and disjoint for the length of the copy.
                unsafe { std::ptr::copy_nonoverlapping(ptr, replacement, layout.size().min(new_size)) };
            }
            replacement
        }

        /// Records a successful change before the old block can be reused.
        ///
        /// # Safety
        /// The pointer and size satisfy the `reallocate` contract. `record` must not
        /// unwind or invalidate either block.
        unsafe fn reallocate_recorded(&self, ptr: *mut u8, layout: Layout, new_size: usize, record: impl FnOnce(*mut u8)) -> *mut u8 {
            // SAFETY: The caller supplies a live block and a valid resize request.
            let replacement = unsafe { self.prepare_reallocation(ptr, layout, new_size) };
            if replacement.is_null() {
                return replacement;
            }
            if replacement != ptr || new_size != layout.size() {
                record(replacement);
            }
            if replacement != ptr {
                // SAFETY: Preparation copied the payload and kept the old block live until recording finished.
                unsafe { self.deallocate(ptr, layout) };
            }
            self.publish_state();
            replacement
        }

        fn publish_state(&self);
    }
}
