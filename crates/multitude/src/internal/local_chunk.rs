// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Single-threaded reference-counted arena chunk.

// All methods on chunks that touch raw memory are themselves `unsafe fn`s
// with documented safety contracts at the function level. Wrapping each line
// of their body in an additional `unsafe { ... }` block adds noise without
// adding any safety boundary, so we let edition-2024's lint slide here.
#![allow(unsafe_op_in_unsafe_fn, reason = "see module doc: inner unsafe blocks in unsafe fn add noise here")]
#![allow(clippy::unnecessary_safety_comment, reason = "safety rationale documented at function level")]

use core::alloc::Layout;
use core::cell::{Cell, UnsafeCell};
use core::mem;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{AllocError, Allocator};

use super::chunk::Chunk;
use super::chunk_provider::ChunkProvider;
use super::drop_entry::replay_drops;

/// A bump-allocation chunk used by a single arena thread.
///
/// The chunk is laid out as a fixed-size header immediately followed in
/// memory by `capacity` bytes of payload. The header type is `Sized` so it
/// can be referenced via thin `NonNull` pointers; payload addresses are
/// recovered with `payload_ptr`.
///
/// # Provider back-pointer
///
/// `provider` is a non-owning raw pointer rather than a `Weak<ChunkProvider>`.
/// This is sound because a `LocalChunk` is single-owner (its refcount is only
/// ever 0 or 1; [`Chunk::inc_ref`] is `unreachable!()`) and reachable only via
/// the owning [`Arena`](crate::Arena)'s `current_local` / `retired_local` /
/// the provider's own `local_cache`. The arena's `provider: Arc<ChunkProvider>`
/// field is declared **after** the chunk-holding fields, so when the arena is
/// dropped the local mutators tear down first while the provider is still
/// live; chunks in the cache are destroyed directly from the provider's own
/// `Drop` (`drain_all`) without going through the back-pointer. The provider
/// therefore strictly outlives every local-chunk teardown that dereferences
/// this pointer, removing the need for a Weak refcount and the dead "orphan"
/// branch the upgrade used to require.
#[repr(C)]
pub(crate) struct LocalChunk<A: Allocator + Clone> {
    /// Non-owning back-pointer to the chunk's provider. See the type-level
    /// doc for the soundness argument. Never dereferenced from
    /// [`Self::destroy`] (the caller — provider methods or the provider's
    /// own drop — supplies the allocator); only read from
    /// [`ChunkOps::teardown_and_release`](super::chunk_ops::ChunkOps::teardown_and_release)
    /// to route the chunk back to the cache.
    provider: *const ChunkProvider<A>,
    capacity: usize,
    ref_count: Cell<u8>,
    drop_entry_count: Cell<u16>,
    /// Explicit padding so the header size stays a multiple of 8, keeping
    /// the payload start 8-aligned. The payload start must be 8-aligned both
    /// for the cache link stored there while the chunk is free (see
    /// [`read_cached_next`](Self::read_cached_next)) and for the `DropEntry`s
    /// the bump allocator packs against the payload tail (which are positioned
    /// relative to the payload start; see [`replay_drops`](super::drop_entry::replay_drops)).
    /// Without it, shrinking `ref_count`/`drop_entry_count` below `usize` would
    /// land the payload at a non-8-aligned offset, which is UB. This is
    /// temporary: once those payload-relative accesses are made tolerant of an
    /// unaligned payload base, this padding can be removed and the header shrunk
    /// from 24 to 20 bytes.
    _padding: [u8; 4],
    /// Bump-payload tail. `data.len() == capacity`. Declared as
    /// `[UnsafeCell<u8>]` (same layout as `[u8]`) so that shared
    /// borrows of the chunk allow interior-mutable writes into the
    /// payload, and so that `NonNull<LocalChunk<A>>` is a **fat
    /// pointer** carrying provenance over the full chunk allocation
    /// (essential for Miri's Stacked / Tree Borrows: a sized-struct
    /// header pointer would have provenance for only the header bytes,
    /// making any payload-derivation undefined behavior).
    data: [UnsafeCell<u8>],
}

// SAFETY: `LocalChunk` would auto-derive `Send` when `A: Send` but for the
// raw `*const ChunkProvider<A>` back-pointer, which the compiler conservatively
// treats as `!Send`. The pointer references a `ChunkProvider<A>` that is owned
// by the same `Arena` that owns this chunk (via `Arc<ChunkProvider<A>>`), so
// moving the arena between threads moves both the chunk and its provider
// together: the address stays valid and the data behind it is `Send` (asserted
// by the `Send` impl on `ChunkProvider<A>` when `A: Send`).
unsafe impl<A: Allocator + Clone + Send> Send for LocalChunk<A> {}

impl<A: Allocator + Clone> LocalChunk<A> {
    /// Size in bytes of the chunk header (everything before the payload).
    #[inline]
    pub(crate) const fn header_size() -> usize {
        // The slice tail has align 1 so sits flush against
        // `drop_entry_count`; computing via the last fixed-size field's
        // offset + size avoids relying on `offset_of!` for DST tails.
        mem::offset_of!(Self, _padding) + mem::size_of::<[u8; 4]>()
    }

    /// Alignment to use when allocating/deallocating a chunk's backing memory.
    /// `A` is no longer stored in the chunk header, so we only need to honour
    /// the alignment of the header fields (max is `usize`, 8 bytes on
    /// 64-bit). The chunk pointer therefore doesn't need to be over-aligned
    /// for `A`.
    #[inline]
    pub(crate) const fn struct_align() -> usize {
        mem::align_of::<usize>()
    }

    /// Allocates a fresh chunk with `payload_size` payload bytes and
    /// refcount 1.
    ///
    /// `allocator` is borrowed only to perform the actual allocation; it is
    /// not stored. `provider` is stashed as a non-owning back-pointer (see
    /// the type-level doc for the soundness argument); pass `ptr::null()`
    /// for stand-alone chunks that will be destroyed directly via
    /// [`Self::destroy`] without going through
    /// [`teardown_and_release`](super::chunk_ops::ChunkOps::teardown_and_release).
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "raw_u8_ptr came from `allocator.allocate(layout)` with `Self`'s alignment; the *mut [u8] -> *mut Self cast preserves the byte address with its full provenance"
    )]
    // Mutation testing is suppressed: `> → >=` only differs at the
    // unreachable exact-`isize::MAX` boundary.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn allocate(allocator: &A, provider: *const ChunkProvider<A>, payload_size: usize) -> Result<NonNull<Self>, AllocError> {
        let (raw_u8_ptr, _layout) =
            crate::internal::chunk_alloc::alloc_chunk_raw(allocator, Self::header_size(), Self::struct_align(), payload_size)?;
        // Construct the fat DST pointer with slice metadata = payload_size.
        // Its data field is `raw_u8_ptr` (carrying full allocation
        // provenance), so the resulting `*mut Self` has provenance over
        // the entire `header + payload` allocation.
        let fat: *mut Self = ptr::slice_from_raw_parts_mut(raw_u8_ptr, payload_size) as *mut Self;
        // SAFETY: `fat` points at freshly-allocated storage aligned for
        // `Self`'s header. Initialize each header field through a
        // projected raw pointer; the slice tail bytes stay uninitialized
        // and are populated by the bump allocator.
        unsafe {
            ptr::write(&raw mut (*fat).provider, provider);
            ptr::write(&raw mut (*fat).capacity, payload_size);
            ptr::write(&raw mut (*fat).ref_count, Cell::new(1));
            ptr::write(&raw mut (*fat).drop_entry_count, Cell::new(0));
            Ok(NonNull::new_unchecked(fat))
        }
    }

    /// Non-owning back-pointer to the chunk's provider. See the type-level
    /// doc for the soundness argument: the provider strictly outlives every
    /// teardown that calls this. Only used by
    /// [`ChunkOps::teardown_and_release`](super::chunk_ops::ChunkOps::teardown_and_release)
    /// to route the chunk back to the cache.
    #[inline]
    pub(crate) fn provider(&self) -> *const ChunkProvider<A> {
        self.provider
    }

    /// Pointer to the first byte of the chunk's payload.
    ///
    /// # Safety
    ///
    /// `chunk` must reference a live (still allocated) chunk.
    #[inline]
    pub(crate) unsafe fn payload_ptr(chunk: NonNull<Self>) -> NonNull<u8> {
        // Project through the DST's slice-tail field directly. This
        // avoids the fat-to-thin cast (`chunk.as_ptr().cast::<u8>()`)
        // whose provenance treatment in Miri is inconsistent — going
        // through `&raw mut (*chunk).data` keeps the slice's provenance
        // intact (covers payload_size bytes).
        let data_slice_ptr: *mut [UnsafeCell<u8>] = &raw mut (*chunk.as_ptr()).data;
        // SAFETY: `data_slice_ptr` is non-null and points at the first
        // payload byte.
        NonNull::new_unchecked(data_slice_ptr.cast::<u8>())
    }

    /// Reconstructs the fat DST `*mut Self` pointer from a thin
    /// `*mut u8` header pointer by reading the chunk's `capacity` field.
    ///
    /// # Safety
    ///
    /// `header` must carry full chunk-allocation provenance.
    #[inline]
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "chunk header is over-aligned for usize per allocate(); capacity field offset is a multiple of usize alignment"
    )]
    pub(crate) unsafe fn header_to_fat(header: *mut u8) -> *mut Self {
        let cap_field_offset = mem::offset_of!(Self, capacity);
        let cap = ptr::read(header.add(cap_field_offset).cast::<usize>());
        ptr::slice_from_raw_parts_mut(header, cap) as *mut Self
    }

    /// Replays drop entries (if any) and deallocates the chunk's backing
    /// memory using the supplied allocator.
    ///
    /// # Safety
    ///
    /// `chunk` must have refcount zero and the caller must hold the unique
    /// remaining reference to it. `allocator` must be functionally
    /// equivalent (i.e. capable of deallocating storage from) the one
    /// passed to [`Self::allocate`] when this chunk was created.
    pub(crate) unsafe fn destroy(chunk: NonNull<Self>, allocator: &A) {
        let header = Self::header_size();
        let align = Self::struct_align();
        // SAFETY: caller owns the only reference; we read trivial fields,
        // replay drops in the payload, then deallocate using the caller-
        // supplied allocator. The pair `(raw_ptr, layout)` exactly matches
        // the one returned by `allocator.allocate` in `allocate`. The
        // header carries no Drop-implementing field (the provider
        // back-pointer is a plain raw pointer), so nothing else needs to
        // be dropped in place before deallocation.
        let header_ref = &*chunk.as_ptr();
        let capacity = header_ref.capacity;
        let drop_count = header_ref.drop_entry_count.get() as usize;
        replay_drops(Self::payload_ptr(chunk).as_ptr(), capacity, drop_count);
        let total = header + capacity;
        let layout = Layout::from_size_align(total, align).expect("matches allocate(); header+capacity stayed within isize::MAX");
        let raw_ptr = chunk.as_ptr().cast::<u8>();
        allocator.deallocate(NonNull::new_unchecked(raw_ptr), layout);
    }

    /// Reads the next-pointer of a cached chunk (stored in the first
    /// bytes of the payload while the chunk lives on a free list).
    /// Returns a thin `*mut u8` header pointer; cache stores thin
    /// pointers since `*mut Self` is fat for the DST.
    ///
    /// # Safety
    ///
    /// Chunk must be in the "cached" state (refcount zero, exclusively
    /// owned by the cache).
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "payload base is usize-aligned (header padded to keep payload 8-aligned); cache link fits within that alignment"
    )]
    #[inline]
    pub(crate) unsafe fn read_cached_next(chunk: NonNull<Self>) -> *mut u8 {
        ptr::read(Self::payload_ptr(chunk).as_ptr().cast::<*mut u8>())
    }

    /// Writes the next-pointer of a cached chunk.
    ///
    /// # Safety
    ///
    /// Same as [`read_cached_next`](Self::read_cached_next).
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "payload base is usize-aligned (header padded to keep payload 8-aligned); cache link fits within that alignment"
    )]
    #[inline]
    pub(crate) unsafe fn write_cached_next(chunk: NonNull<Self>, next: *mut u8) {
        ptr::write(Self::payload_ptr(chunk).as_ptr().cast::<*mut u8>(), next);
    }

    /// Re-initializes a chunk popped from the cache: refcount → 1,
    /// drop-entry count → 0. The caller becomes the +1 holder.
    ///
    /// # Safety
    ///
    /// `chunk` must be a freshly popped, refcount-zero, uniquely-owned
    /// chunk (cache invariant); the cache link is invalidated by this call.
    #[inline]
    pub(crate) unsafe fn reinit_for_acquire(chunk: NonNull<Self>) {
        // SAFETY: caller owns the unique reference; refcount and drop count
        // are trivially-typed cells.
        let r = &*chunk.as_ptr();
        r.ref_count.set(1);
        r.drop_entry_count.set(0);
    }

    /// Overwrites the refcount. Test-only seam so unit tests can drive
    /// refcount-dependent paths without poking the field directly.
    #[cfg(test)]
    pub(crate) fn set_ref_count_for_test(&self, count: u8) {
        self.ref_count.set(count);
    }
}

/// Largest payload byte count a local chunk can offer to a bump allocator
/// after accounting for the header.
#[inline]
#[must_use]
pub(crate) const fn max_bump_extent<A: Allocator + Clone>() -> usize {
    super::constants::MAX_CHUNK_BYTES - LocalChunk::<A>::header_size()
}

impl<A: Allocator + Clone> Chunk for LocalChunk<A> {
    #[inline]
    fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn inc_ref(&self) {
        // Local chunks host arena-lifetime allocations, which are single-owner:
        // the arena holds the sole +1 and plain arena allocations hand back
        // borrows without cloning the refcount. Only smart pointers (Arc/Box)
        // clone a chunk reference, and those live exclusively in `SharedChunk`.
        // So this is never reached in production; the `Chunk` trait only
        // requires it to keep the local/shared chunk machinery uniform.
        unreachable!("LocalChunk refcount is never incremented; smart pointers use SharedChunk")
    }

    #[inline]
    fn dec_ref(&self) -> bool {
        let new = self.ref_count.get() - 1;
        self.ref_count.set(new);
        new == 0
    }

    #[inline]
    fn drop_entry_count(&self) -> usize {
        self.drop_entry_count.get() as usize
    }

    #[inline]
    fn set_drop_entry_count(&self, count: usize) {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a 64KiB chunk holds at most 4096 drop entries (« u16::MAX); round-trip asserted below"
        )]
        let narrowed = count as u16;
        debug_assert_eq!(usize::from(narrowed), count, "drop-entry count exceeds u16 range");
        self.drop_entry_count.set(narrowed);
    }
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    /// `struct_align` returns `align_of::<usize>()` (the largest alignment
    /// of any header field) regardless of `A` — the chunk no longer stores
    /// an allocator copy.
    #[test]
    fn struct_align_matches_usize() {
        assert_eq!(LocalChunk::<Global>::struct_align(), mem::align_of::<usize>());
    }

    /// `max_bump_extent` subtracts the header from `MAX_CHUNK_BYTES`;
    /// pin the relation so `- → +` mutation (which would balloon the
    /// result past the real allocation) is caught.
    #[test]
    fn max_bump_extent_is_max_minus_header() {
        let header = LocalChunk::<Global>::header_size();
        let extent = max_bump_extent::<Global>();
        assert_eq!(extent, super::super::constants::MAX_CHUNK_BYTES - header);
        // Extent strictly less than MAX_CHUNK_BYTES because header is non-zero.
        assert!(extent < super::super::constants::MAX_CHUNK_BYTES);
    }

    /// `dec_ref` returns `true` only when the count hits zero. It is
    /// exercised indirectly by chunk teardown, but a direct unit test
    /// guarantees the boolean return value is `false` for non-last
    /// drops and `true` only on the final decrement. (`inc_ref` is
    /// unreachable for local chunks — see its impl — so the elevated
    /// count is set directly.)
    #[test]
    fn dec_ref_returns_true_only_on_final_release() {
        // Build a one-shot chunk so we can poke `ref_count` directly.
        let chunk = LocalChunk::<Global>::allocate(&Global, ptr::null(), 64).expect("alloc");
        // SAFETY: `chunk` is the unique owner; we just allocated it.
        unsafe {
            let c = chunk.as_ref();
            // refcount starts at 1 from allocate(); simulate a second holder.
            c.set_ref_count_for_test(2);
            assert!(!c.dec_ref(), "dec from 2 leaves 1");
            // Use destroy to release without going through the provider.
            assert!(c.dec_ref(), "dec from 1 hits zero and returns true");
            LocalChunk::destroy(chunk, &Global);
        }
    }

    /// `header_size` is `offset_of!(_padding) + size_of::<[u8; 4]>()`. For
    /// `LocalChunk<Global>`, the header layout is fixed: 8 (provider) +
    /// 8 (capacity) + 1 (`ref_count`) + 1 pad + 2 (`drop_entry_count`) +
    /// 4 (`_padding`) = 24 bytes. Pinning the exact value catches an
    /// arithmetic-operator mutation (`+ → *`) that would silently shift the
    /// payload base.
    #[test]
    fn header_size_for_global_is_24() {
        assert_eq!(LocalChunk::<Global>::header_size(), 24);
    }

    /// `Chunk::inc_ref` on a local chunk is unreachable in production — local
    /// chunks have at most one owner (the arena). The trait impl exists only
    /// to keep the `Chunk` interface uniform between local and shared chunks;
    /// invoking it must abort/panic so that any future caller that wrongly
    /// routes a local refcount bump through this path fails loudly. A test
    /// invoking the trait method and expecting a panic kills a mutant that
    /// replaces the body with `()`.
    #[test]
    #[should_panic(expected = "LocalChunk refcount is never incremented")]
    fn local_chunk_inc_ref_is_unreachable() {
        let chunk = LocalChunk::<Global>::allocate(&Global, ptr::null(), 64).expect("alloc");
        // SAFETY: unique owner from a fresh allocation. Catch the
        // expected panic so we can destroy the chunk before resuming
        // unwinding; without this, Miri reports the chunk as leaked.
        unsafe {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                <LocalChunk<Global> as Chunk>::inc_ref(chunk.as_ref());
            }));
            LocalChunk::destroy(chunk, &Global);
            std::panic::resume_unwind(result.expect_err("inc_ref must panic"));
        }
    }
}
