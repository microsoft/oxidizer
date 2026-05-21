// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::alloc::Layout;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use crate::Arena;
use crate::internal::in_chunk::InLocalChunk;
use crate::internal::local_chunk::LocalChunk;

/// `&Arena<A>` is the allocator handle: cheap to copy and backed by
/// local chunks. `allocate` bumps the chunk refcount; `deallocate`
/// releases it.
///
/// # Safety
///
/// `deallocate` must get a pointer returned by `allocate` on the same
/// arena. The chunk-header mask would misidentify a foreign pointer.
// SAFETY: the chunk refcount keeps each allocation alive until the
// matching `deallocate`.
unsafe impl<A: Allocator + Clone> Allocator for &Arena<A> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let ptr = self.allocate_layout(layout)?;
        let fat: *mut [u8] = core::ptr::slice_from_raw_parts_mut(ptr.as_ptr(), layout.size());
        // SAFETY: `ptr.as_ptr()` is non-null.
        Ok(unsafe { NonNull::new_unchecked(fat) })
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, _layout: Layout) {
        // SAFETY: chunk-header invariant — `ptr` was returned by
        // `allocate` above, which only hands out pointers into chunks
        // produced by this arena.
        let in_chunk = unsafe { InLocalChunk::<_, A>::new(ptr) };
        let chunk = in_chunk.chunk_ptr();
        // SAFETY: refcount-positive — `allocate` left a +1 for this
        // pointer.
        unsafe { LocalChunk::dec_ref(chunk) };
    }

    unsafe fn grow(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Grow in place when the buffer still reaches the bump cursor;
        // otherwise fall back to allocate-copy-deallocate.
        if old_layout.align() == new_layout.align() && new_layout.size() >= old_layout.size() {
            // SAFETY: `ptr` was previously returned by `self.allocate`
            // and `old_layout` is the layout it was allocated with.
            if let Some(grown) = unsafe { self.try_grow_in_place(ptr, old_layout, new_layout) } {
                let fat: *mut [u8] = core::ptr::slice_from_raw_parts_mut(grown.as_ptr(), new_layout.size());
                // SAFETY: `grown.as_ptr()` is non-null.
                return Ok(unsafe { NonNull::new_unchecked(fat) });
            }
        }
        let new_ptr = self.allocate(new_layout)?;
        // SAFETY: `ptr` is valid for old_layout.size() bytes; new_ptr
        // is fresh and has at least new_layout.size() bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_ptr().cast::<u8>(), old_layout.size());
            self.deallocate(ptr, old_layout);
        }
        self.bump_relocation();
        Ok(new_ptr)
    }
}
