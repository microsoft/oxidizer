// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::alloc::Layout;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{AllocError, Allocator};

use crate::Arena;
use crate::arena::alloc_value::acquire_shared_chunk_ref;
use crate::internal::chunk_ref::ChunkRef;
use crate::internal::constants::max_smart_ptr_align;

/// Maximum `layout.align()` accepted by `Allocator::allocate`: the
/// returned pointer must lie strictly inside the first `CHUNK_ALIGN`
/// bytes of its chunk so the header-recovery mask used by
/// `deallocate` can recover the chunk pointer.
const MAX_SMART_PTR_ALIGN: usize = max_smart_ptr_align();

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
        // Route through the shared-chunk path so the returned pointer can
        // outlive the current mutator borrow via a per-allocation +1 refcount.
        // Reject alignments at/above the smart-pointer ceiling (as `alloc_box` /
        // `alloc_arc` do): the header-from-mask helper requires the value to lie
        // strictly inside the first `CHUNK_ALIGN` bytes of the chunk.
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            return Err(AllocError);
        }
        // Zero-byte allocations need a non-null, well-aligned pointer but no
        // chunk state (mirroring `std::alloc::Global`). `without_provenance_mut`
        // keeps it strict-provenance-clean — ZSTs are never dereferenced.
        if layout.size() == 0 {
            // SAFETY: `layout.align()` is a non-zero power of two.
            let dangling = unsafe { NonNull::new_unchecked(ptr::without_provenance_mut::<u8>(layout.align())) };
            return Ok(NonNull::slice_from_raw_parts(dangling, 0));
        }
        // Refill / oversized hint includes one alignment slack so
        // `try_alloc(size, align)` always succeeds inside a chunk sized
        // for this allocation, regardless of the bump cursor's pre-alignment.
        let refill_hint = layout.size().saturating_add(layout.align());
        loop {
            if let Some((slot, chunk_ptr)) = self.current_shared().try_alloc_with_chunk(layout.size(), layout.align()) {
                let chunk_ref = self.acquire_current_shared_chunk_ref(chunk_ptr);
                let ptr = slot.as_non_null();
                let _ = chunk_ref.forget();
                return Ok(NonNull::slice_from_raw_parts(ptr, layout.size()));
            }
            if self.is_oversized_shared(refill_hint) {
                return self.alloc_oversized_shared_with(refill_hint, |mutator, chunk_ptr| {
                    let (slot, _chunk) = mutator
                        .try_alloc_with_chunk(layout.size(), layout.align())
                        .expect("dedicated oversized chunk sized to fit allocation + alignment slack");
                    let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                    let ptr = slot.as_non_null();
                    let _ = chunk_ref.forget();
                    NonNull::slice_from_raw_parts(ptr, layout.size())
                });
            }
            self.refill_shared(refill_hint)?;
        }
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // Zero-byte allocations don't own any chunk refcount (see
        // `allocate`); nothing to release.
        if layout.size() == 0 {
            return;
        }
        // SAFETY: caller guarantees `ptr` was returned by `Self::allocate`
        // on the same arena, so it is hosted in a `SharedChunk<A>` we hold
        // a +1 strong reference on. `ChunkRef::from_value_ptr` adopts that
        // +1 and releases it on drop.
        let _ref: ChunkRef<A> = unsafe { ChunkRef::from_value_ptr(ptr) };
    }

    unsafe fn grow(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Bump-allocators can't reliably extend in place across chunks
        // and don't need to: fall back to allocate-copy-deallocate. The
        // new allocation acquires its own +1 chunk refcount; the old
        // refcount is released by `deallocate` below. We copy only the
        // overlapping prefix (`min(old, new)`) so the fallback stays
        // sound even if a caller passes a smaller `new_layout`.
        let new = self.allocate(new_layout)?;
        let copy_bytes = old_layout.size().min(new_layout.size());
        if copy_bytes != 0 {
            // SAFETY: the old allocation is initialized for
            // `old_layout.size()` bytes and the new allocation has
            // `new_layout.size()` bytes; copying their `min` stays in
            // bounds of both. The new allocation is non-overlapping
            // arena storage we just acquired.
            unsafe {
                ptr::copy_nonoverlapping(ptr.as_ptr(), new.cast::<u8>().as_ptr(), copy_bytes);
            }
        }
        // SAFETY: caller upholds `deallocate`'s contract for `ptr`.
        unsafe { self.deallocate(ptr, old_layout) };
        Ok(new)
    }
}
