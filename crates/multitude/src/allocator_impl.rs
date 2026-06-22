// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::alloc::Layout;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{AllocError, Allocator};

use crate::Arena;
use crate::arena::alloc_value::acquire_chunk_ref;
use crate::internal::chunk_ref::ChunkRef;
use crate::internal::constants::max_smart_ptr_align;

/// Maximum `layout.align()` accepted by `Allocator::allocate`: the
/// returned pointer must lie strictly inside the first `CHUNK_ALIGN`
/// bytes of its chunk so the header-recovery mask used by
/// `deallocate` can recover the chunk pointer.
const MAX_SMART_PTR_ALIGN: usize = max_smart_ptr_align();

/// `&Arena<A>` is the allocator handle: cheap to copy and backed by
/// chunks. `allocate` bumps the chunk refcount; `deallocate`
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
        // Route through the chunk path so the returned pointer can
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
            if let Some((slot, chunk_ptr)) = self.current().try_alloc_with_chunk(layout.size(), layout.align()) {
                let chunk_ref = self.acquire_current_chunk_ref(chunk_ptr);
                let ptr = slot.as_non_null();
                let _ = chunk_ref.forget();
                return Ok(NonNull::slice_from_raw_parts(ptr, layout.size()));
            }
            if self.is_oversized(refill_hint) {
                return self.alloc_oversized_shared_with(refill_hint, |mutator, chunk_ptr| {
                    let (slot, _chunk) = mutator
                        .try_alloc_with_chunk(layout.size(), layout.align())
                        .expect("dedicated oversized chunk sized to fit allocation + alignment slack");
                    let chunk_ref = acquire_chunk_ref::<A>(chunk_ptr);
                    let ptr = slot.as_non_null();
                    let _ = chunk_ref.forget();
                    NonNull::slice_from_raw_parts(ptr, layout.size())
                });
            }
            self.refill(refill_hint)?;
        }
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // Zero-byte allocations don't own any chunk refcount (see
        // `allocate`); nothing to release.
        if layout.size() == 0 {
            return;
        }
        // SAFETY: caller guarantees `ptr` was returned by `Self::allocate`
        // on the same arena, so it is hosted in a `Chunk<A>` we hold
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

/// Legacy `allocator-api2` 0.2 `Allocator` impl, so `&Arena<A>` can directly
/// back collections from crates (notably `hashbrown`) that have not yet moved
/// to the modern allocator API. Every method forwards verbatim to the 0.4 impl
/// above; when those crates upgrade, this impl can simply be deleted.
// SAFETY: forwards to the 0.4 `Allocator` impl; the 0.2 and 0.4 trait contracts
// are identical, and the only version-specific type (`AllocError`) is a
// zero-payload marker.
unsafe impl<A: Allocator + Clone> allocator_api2_02::alloc::Allocator for &Arena<A> {
    #[inline]
    #[allow(clippy::map_err_ignore, reason = "AllocError carries no payload; only the variant is bridged")]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, allocator_api2_02::alloc::AllocError> {
        <&Arena<A> as Allocator>::allocate(self, layout).map_err(|_| allocator_api2_02::alloc::AllocError)
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded to the 0.4 impl under the same contract.
        unsafe { <&Arena<A> as Allocator>::deallocate(self, ptr, layout) };
    }

    #[inline]
    #[allow(clippy::map_err_ignore, reason = "AllocError carries no payload; only the variant is bridged")]
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, allocator_api2_02::alloc::AllocError> {
        // SAFETY: forwarded to the 0.4 impl under the same contract.
        unsafe { <&Arena<A> as Allocator>::grow(self, ptr, old_layout, new_layout) }.map_err(|_| allocator_api2_02::alloc::AllocError)
    }
}

#[cfg(test)]
mod tests {
    use core::alloc::Layout;

    use allocator_api2_02::alloc::Allocator as LegacyAllocator;

    use crate::Arena;

    // Exercises the legacy (`allocator-api2` 0.2) `Allocator` impl directly,
    // independent of the optional `hashbrown` feature: allocate, grow, and
    // deallocate, plus the rejected-alignment error arm.
    #[test]
    fn arena_backs_legacy_allocator_api2() {
        let arena = Arena::new();
        let handle: &Arena = &arena;
        let layout = Layout::from_size_align(64, 8).unwrap();
        let p = LegacyAllocator::allocate(&handle, layout).expect("legacy allocate");
        let new_layout = Layout::from_size_align(128, 8).unwrap();
        // SAFETY: `p` came from `allocate` with `layout`, and `new_layout` is larger.
        let p = unsafe { LegacyAllocator::grow(&handle, p.cast::<u8>(), layout, new_layout) }.expect("legacy grow");
        // SAFETY: `p` came from `grow` with `new_layout`.
        unsafe { LegacyAllocator::deallocate(&handle, p.cast::<u8>(), new_layout) };

        // An alignment at/above the smart-pointer ceiling is rejected as a
        // recoverable error through the legacy impl's `map_err` arm.
        let over_aligned = Layout::from_size_align(8, super::MAX_SMART_PTR_ALIGN).unwrap();
        LegacyAllocator::allocate(&handle, over_aligned).expect_err("over-aligned request must be rejected");
    }
}
