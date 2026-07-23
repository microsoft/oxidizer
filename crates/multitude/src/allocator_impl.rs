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
                return self
                    .alloc_oversized_shared_with(refill_hint, |mutator, chunk_ptr| {
                        let (slot, _chunk) = mutator
                            .try_alloc_with_chunk(layout.size(), layout.align())
                            .expect("dedicated oversized chunk sized to fit allocation + alignment slack");
                        let chunk_ref = acquire_chunk_ref::<A>(chunk_ptr);
                        let ptr = slot.as_non_null();
                        let _ = chunk_ref.forget();
                        NonNull::slice_from_raw_parts(ptr, layout.size())
                    })
                    .map_err(Into::into);
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
        // The most recent allocation in the current chunk can grow by simply
        // advancing the bump cursor. Keep the same alignment requirement so
        // the original pointer is guaranteed to satisfy `new_layout`.
        if new_layout.align() == old_layout.align()
            && self.try_grow_local_in_place(ptr.as_ptr() as usize, old_layout.size(), new_layout.size())
        {
            return Ok(NonNull::slice_from_raw_parts(ptr, new_layout.size()));
        }

        // Otherwise allocate-copy-deallocate. The new allocation acquires its
        // own +1 chunk refcount; the old refcount is released by `deallocate`
        // below.
        let new = self.allocate(new_layout)?;
        if old_layout.size() != 0 {
            // SAFETY: the old allocation is initialized for
            // `old_layout.size()` bytes, and the grow contract guarantees the
            // new allocation is at least that large. The allocations do not
            // overlap.
            unsafe {
                ptr::copy_nonoverlapping(ptr.as_ptr(), new.cast::<u8>().as_ptr(), old_layout.size());
            }
        }
        // SAFETY: caller upholds `deallocate`'s contract for `ptr`.
        unsafe { self.deallocate(ptr, old_layout) };
        Ok(new)
    }

    unsafe fn grow_zeroed(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if new_layout.align() == old_layout.align()
            && self.try_grow_local_in_place(ptr.as_ptr() as usize, old_layout.size(), new_layout.size())
        {
            // SAFETY: successful in-place growth extended this allocation
            // through `new_layout.size()`; only the newly exposed tail is
            // written.
            unsafe {
                ptr.as_ptr()
                    .add(old_layout.size())
                    .write_bytes(0, new_layout.size() - old_layout.size());
            }
            return Ok(NonNull::slice_from_raw_parts(ptr, new_layout.size()));
        }

        let new = self.allocate_zeroed(new_layout)?;
        // SAFETY: the grow contract guarantees the new block is at least as
        // large as the old block; the allocations do not overlap.
        unsafe {
            ptr::copy_nonoverlapping(ptr.as_ptr(), new.cast::<u8>().as_ptr(), old_layout.size());
            self.deallocate(ptr, old_layout);
        }
        Ok(new)
    }

    unsafe fn shrink(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Keeping a larger bump block is legal: `new_layout` fits it, and
        // deallocation does not require the original size. Avoiding a fresh
        // allocation also avoids abandoning both the old and copied blocks.
        if new_layout.size() != 0 && new_layout.align() == old_layout.align() {
            return Ok(NonNull::slice_from_raw_parts(ptr, new_layout.size()));
        }

        let new = self.allocate(new_layout)?;
        if new_layout.size() != 0 {
            // SAFETY: the shrink contract guarantees the old block covers
            // `new_layout.size()` bytes; the allocations do not overlap.
            unsafe {
                ptr::copy_nonoverlapping(ptr.as_ptr(), new.cast::<u8>().as_ptr(), new_layout.size());
            }
        }
        // SAFETY: caller upholds `deallocate`'s contract for `ptr`.
        unsafe { self.deallocate(ptr, old_layout) };
        Ok(new)
    }
}

/// `allocator-api2` 0.2 compatibility for arena-backed collections such as
/// `hashbrown`.
// SAFETY: forwards to the 0.4 `Allocator` impl; the 0.2 and 0.4 trait contracts
// are identical, and the only version-specific type (`AllocError`) is a
// zero-payload marker.
unsafe impl<A: Allocator + Clone> allocator_api2_02::alloc::Allocator for &Arena<A> {
    #[inline]
    #[expect(clippy::map_err_ignore, reason = "AllocError carries no payload; only the variant is bridged")]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, allocator_api2_02::alloc::AllocError> {
        <&Arena<A> as Allocator>::allocate(self, layout).map_err(|_| allocator_api2_02::alloc::AllocError)
    }

    #[inline]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: forwarded to the 0.4 impl under the same contract.
        unsafe { <&Arena<A> as Allocator>::deallocate(self, ptr, layout) };
    }

    #[inline]
    #[expect(clippy::map_err_ignore, reason = "AllocError carries no payload; only the variant is bridged")]
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, allocator_api2_02::alloc::AllocError> {
        // SAFETY: forwarded to the 0.4 impl under the same contract.
        unsafe { <&Arena<A> as Allocator>::grow(self, ptr, old_layout, new_layout) }.map_err(|_| allocator_api2_02::alloc::AllocError)
    }

    #[inline]
    #[expect(clippy::map_err_ignore, reason = "AllocError carries no payload; only the variant is bridged")]
    unsafe fn grow_zeroed(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, allocator_api2_02::alloc::AllocError> {
        // SAFETY: forwarded to the 0.4 impl under the same contract.
        unsafe { <&Arena<A> as Allocator>::grow_zeroed(self, ptr, old_layout, new_layout) }
            .map_err(|_| allocator_api2_02::alloc::AllocError)
    }

    #[inline]
    #[expect(clippy::map_err_ignore, reason = "AllocError carries no payload; only the variant is bridged")]
    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, allocator_api2_02::alloc::AllocError> {
        // SAFETY: forwarded to the 0.4 impl under the same contract.
        unsafe { <&Arena<A> as Allocator>::shrink(self, ptr, old_layout, new_layout) }.map_err(|_| allocator_api2_02::alloc::AllocError)
    }
}

#[cfg(test)]
mod tests {
    use core::alloc::Layout;

    use allocator_api2_02::alloc::Allocator as LegacyAllocator;

    use crate::Arena;

    // Exercises the complete `allocator-api2` 0.2 interface directly.
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

        let old_layout = Layout::from_size_align(8, 8).unwrap();
        let p = LegacyAllocator::allocate(&handle, old_layout).expect("legacy allocate for zeroed growth");
        // SAFETY: `p` addresses the eight bytes returned above.
        unsafe { p.cast::<u8>().as_ptr().write_bytes(0xA5, old_layout.size()) };
        let grown_layout = Layout::from_size_align(32, 16).unwrap();
        // SAFETY: `p` came from `allocate` with `old_layout`, and
        // `grown_layout` is larger and suitably aligned.
        let p = unsafe { LegacyAllocator::grow_zeroed(&handle, p.cast::<u8>(), old_layout, grown_layout) }.expect("legacy zeroed growth");
        // SAFETY: `p` addresses the 32 initialized bytes returned above.
        let bytes = unsafe { core::slice::from_raw_parts(p.cast::<u8>().as_ptr(), grown_layout.size()) };
        assert_eq!(&bytes[..old_layout.size()], &[0xA5; 8]);
        assert_eq!(&bytes[old_layout.size()..], &[0; 24]);

        let shrunk_layout = Layout::from_size_align(8, 8).unwrap();
        // SAFETY: `p` came from `grow_zeroed` with `grown_layout`, and
        // `shrunk_layout` fits within it.
        let p = unsafe { LegacyAllocator::shrink(&handle, p.cast::<u8>(), grown_layout, shrunk_layout) }.expect("legacy shrink");
        // SAFETY: `p` addresses the eight initialized bytes retained above.
        let bytes = unsafe { core::slice::from_raw_parts(p.cast::<u8>().as_ptr(), shrunk_layout.size()) };
        assert_eq!(bytes, &[0xA5; 8]);
        // SAFETY: `p` came from `shrink` with `shrunk_layout`.
        unsafe { LegacyAllocator::deallocate(&handle, p.cast::<u8>(), shrunk_layout) };

        let p = LegacyAllocator::allocate(&handle, old_layout).expect("legacy allocate for zero-sized shrink");
        let empty_layout = Layout::from_size_align(0, old_layout.align()).unwrap();
        // SAFETY: `p` came from `allocate` with `old_layout`, and an empty
        // allocation fits within it.
        let p = unsafe { LegacyAllocator::shrink(&handle, p.cast::<u8>(), old_layout, empty_layout) }.expect("legacy zero-sized shrink");
        assert_eq!(p.len(), 0);
        // SAFETY: `p` came from `shrink` with `empty_layout`.
        unsafe { LegacyAllocator::deallocate(&handle, p.cast::<u8>(), empty_layout) };

        // Unsupported alignment remains a recoverable allocator error.
        let over_aligned = Layout::from_size_align(8, super::MAX_SMART_PTR_ALIGN).unwrap();
        LegacyAllocator::allocate(&handle, over_aligned).expect_err("over-aligned request must be rejected");
    }
}
