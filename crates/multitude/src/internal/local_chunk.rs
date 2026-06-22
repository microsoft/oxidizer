// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Single-threaded reference-counted arena chunk.

// Raw-memory methods are `unsafe fn` with item-level safety contracts; inner
// unsafe blocks would not add a boundary here.
#![allow(unsafe_op_in_unsafe_fn, reason = "see module doc: inner unsafe blocks in unsafe fn add noise here")]
#![allow(clippy::unnecessary_safety_comment, reason = "safety rationale documented at function level")]

use core::cell::{Cell, UnsafeCell};
use core::mem;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{AllocError, Allocator};

use super::chunk::Chunk;
use super::chunk_provider::ChunkProvider;
use super::drop_entry::replay_drops;

/// A bump-allocation chunk used by a single arena thread.
///
/// Fixed header followed by `capacity` payload bytes.
///
/// # Provider back-pointer
///
/// `provider` is a non-owning raw pointer. A `LocalChunk` is single-owner and
/// reachable only through the owning arena or provider cache; arena field
/// order keeps the provider alive for all local-chunk teardown paths.
#[repr(C)]
pub(crate) struct LocalChunk<A: Allocator + Clone> {
    /// Non-owning provider back-pointer, used only by
    /// [`ChunkOps::teardown_and_release`](super::chunk_ops::ChunkOps::teardown_and_release).
    provider: *const ChunkProvider<A>,
    capacity: usize,
    /// Intrusive next-link as a thin `*mut u8` header pointer.
    ///
    /// * While the chunk is **retired** (refcount = 1, sitting on
    ///   [`RetiredLocalChunks`](crate::arena::retired_local::RetiredLocalChunks))
    ///   the field links the next retired chunk.
    /// * While the chunk is **cached** (refcount = 0, sitting on the
    ///   provider's local freelist) it links the next cached chunk.
    ///
    /// The phases are mutually exclusive, so one field serves both lists.
    next: Cell<*mut u8>,
    ref_count: Cell<u8>,
    drop_entry_count: Cell<u16>,
    /// Free bytes between the bump cursor and the drop-entry top at the
    /// time this chunk was retired from a `ChunkMutator`. Set in the
    /// mutator's `Drop` and read by [`ChunkProvider::release_local`] to
    /// decrement the wasted-tail counter. Stays at 0 for chunks that
    /// never went through a mutator (e.g. preallocated cache fills).
    #[cfg(feature = "stats")]
    wasted_at_retire: Cell<u32>,
    /// Bump-payload tail. `[UnsafeCell<u8>]` permits payload writes through
    /// shared chunk borrows and keeps fat-pointer provenance over the full
    /// allocation.
    ///
    /// The payload start is **not** required to be `DropEntry`-aligned:
    /// [`replay_drops`](super::drop_entry::replay_drops) computes drop-
    /// entry positions via the absolute payload-end address, so they
    /// remain aligned regardless of where the payload begins.
    data: [UnsafeCell<u8>],
}

// SAFETY: the raw provider back-pointer moves with the arena that owns both
// the chunk and `Arc<ChunkProvider<A>>`; when `A: Send`, the provider data is
// also Send.
unsafe impl<A: Allocator + Clone + Send> Send for LocalChunk<A> {}

impl<A: Allocator + Clone> LocalChunk<A> {
    /// Size in bytes of the chunk header (everything before the payload).
    #[inline]
    // Mutation testing is suppressed: `header_size` is a pure const
    // layout computation, exhaustively pinned by `header_size_for_global_matches_layout`
    // (exact-value assertion under both feature configs), so any arithmetic
    // mutation is already caught by that test. cargo-mutants runs with
    // `all_features = true`, under which the `#[cfg(not(feature = "stats"))]`
    // branch is dead code, so its mutants are structurally unkillable by that
    // run regardless.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) const fn header_size() -> usize {
        // The last fixed-size field's offset + its size; the
        // `[UnsafeCell<u8>]` tail has align 1 so sits flush against it.
        // Under `stats`, `wasted_at_retire` is the last fixed field;
        // otherwise it's `drop_entry_count`.
        #[cfg(feature = "stats")]
        {
            mem::offset_of!(Self, wasted_at_retire) + mem::size_of::<Cell<u32>>()
        }
        #[cfg(not(feature = "stats"))]
        {
            mem::offset_of!(Self, drop_entry_count) + mem::size_of::<Cell<u16>>()
        }
    }

    /// Alignment for the chunk backing allocation.
    ///
    /// Local chunks need no `CHUNK_ALIGN` base alignment; base and value
    /// alignments coincide.
    #[inline]
    pub(crate) const fn struct_align() -> usize {
        Self::value_align()
    }

    /// The chunk type's own alignment (`align_of::<Self>()`, ignoring the
    /// align-1 `[UnsafeCell<u8>]` tail), used to round the allocation size.
    /// Equal to [`Self::struct_align`] for local chunks. Pinned against the
    /// real `align_of_val` by `value_align_matches_real_alignment`.
    #[inline]
    pub(crate) const fn value_align() -> usize {
        mem::align_of::<usize>()
    }

    /// Allocates a fresh chunk with `payload_size` payload bytes and
    /// refcount 1.
    ///
    /// `allocator` is not stored. `provider` may be null for standalone chunks
    /// destroyed directly via [`Self::destroy`].
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "raw_u8_ptr came from `allocator.allocate(layout)` with `Self`'s alignment; the *mut [u8] -> *mut Self cast preserves the byte address with its full provenance"
    )]
    // Mutation testing is suppressed: `> → >=` only differs at the
    // unreachable exact-`isize::MAX` boundary.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn allocate(allocator: &A, provider: *const ChunkProvider<A>, payload_size: usize) -> Result<NonNull<Self>, AllocError> {
        let (raw_u8_ptr, _layout) = crate::internal::chunk_alloc::alloc_chunk_raw(
            allocator,
            Self::header_size(),
            payload_size,
            Self::value_align(),
            Self::struct_align(),
        )?;
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
            ptr::write(&raw mut (*fat).next, Cell::new(ptr::null_mut()));
            #[cfg(feature = "stats")]
            ptr::write(&raw mut (*fat).wasted_at_retire, Cell::new(0));
            Ok(NonNull::new_unchecked(fat))
        }
    }

    /// Non-owning provider back-pointer used by
    /// [`ChunkOps::teardown_and_release`](super::chunk_ops::ChunkOps::teardown_and_release).
    #[inline]
    pub(crate) fn provider(&self) -> *const ChunkProvider<A> {
        self.provider
    }

    /// Reads the free byte count stashed by the owning `ChunkMutator`'s
    /// `Drop` (the gap between bump cursor and drop-entry top at retire).
    /// `0` for chunks that never went through a mutator.
    #[cfg(feature = "stats")]
    #[inline]
    pub(crate) fn wasted_at_retire(&self) -> u32 {
        self.wasted_at_retire.get()
    }

    /// Stashes the chunk's wasted-tail bytes at retire time, to be
    /// subtracted from the provider's wasted-tail counter when the chunk
    /// is eventually released to the cache or destroyed.
    #[cfg(feature = "stats")]
    #[inline]
    pub(crate) fn set_wasted_at_retire(&self, n: u32) {
        self.wasted_at_retire.set(n);
    }

    /// Pointer to the first byte of the chunk's payload.
    ///
    /// # Safety
    ///
    /// `chunk` must reference a live (still allocated) chunk.
    #[inline]
    pub(crate) unsafe fn payload_ptr(chunk: NonNull<Self>) -> NonNull<u8> {
        // Project through the DST tail so the pointer keeps payload provenance.
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
        // SAFETY: caller owns the only reference. We replay payload drops,
        // deallocate with the matching `chunk_layout`, and the header has no
        // Drop fields.
        let header_ref = &*chunk.as_ptr();
        let capacity = header_ref.capacity;
        let drop_count = header_ref.drop_entry_count.get() as usize;
        replay_drops(Self::payload_ptr(chunk).as_ptr(), capacity, drop_count);
        let layout = crate::internal::chunk_alloc::chunk_layout(header, capacity, Self::value_align(), Self::struct_align())
            .expect("matches allocate(); header+capacity stayed within isize::MAX");
        let raw_ptr = chunk.as_ptr().cast::<u8>();
        allocator.deallocate(NonNull::new_unchecked(raw_ptr), layout);
    }

    /// Reads the intrusive next-link as a thin header pointer. The field is
    /// shared by the retired list and provider cache freelist.
    ///
    /// # Safety
    ///
    /// Chunk must be live (allocated) — either retained (refcount ≥ 1)
    /// for the retired-list reader or exclusively cache-owned
    /// (refcount = 0) for the cache reader.
    #[inline]
    pub(crate) unsafe fn next(chunk: NonNull<Self>) -> *mut u8 {
        chunk.as_ref().next.get()
    }

    /// Replaces the intrusive next-link, returning the previous value.
    /// Used by both the retired-list and cache-freelist push paths.
    ///
    /// # Safety
    ///
    /// Same as [`Self::next`]; in addition, the caller must hold
    /// exclusive access to the field for the duration of the call.
    /// `LocalChunk` is `!Sync`, so this is satisfied by the
    /// owning-thread invariant.
    #[inline]
    pub(crate) unsafe fn set_next(chunk: NonNull<Self>, next: *mut u8) -> *mut u8 {
        chunk.as_ref().next.replace(next)
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

    /// Returns the number of drop entries currently stored at the tail of the
    /// chunk.
    #[inline]
    pub(crate) fn drop_entry_count(&self) -> usize {
        self.drop_entry_count.get() as usize
    }

    /// Sets the number of drop entries currently stored at the tail of the
    /// chunk.
    #[inline]
    pub(crate) fn set_drop_entry_count(&self, count: usize) {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a 64KiB chunk holds at most 4096 drop entries (« u16::MAX); round-trip asserted below"
        )]
        let narrowed = count as u16;
        debug_assert_eq!(usize::from(narrowed), count, "drop-entry count exceeds u16 range");
        self.drop_entry_count.set(narrowed);
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
    // Mutation testing is suppressed: returning 0/1 makes every chunk
    // report no usable payload, sending the allocator's bump-fit loop
    // into an unbounded refill spin (the suite hangs rather than fails).
    #[cfg_attr(test, mutants::skip)]
    fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn inc_ref(&self) {
        // Local chunks are single-owner; smart pointers use `SharedChunk`.
        // This exists only to satisfy the shared `Chunk` trait.
        unreachable!("LocalChunk refcount is never incremented; smart pointers use SharedChunk")
    }

    #[inline]
    fn dec_ref(&self) -> bool {
        let new = self.ref_count.get() - 1;
        self.ref_count.set(new);
        new == 0
    }
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    /// `struct_align` returns `align_of::<usize>()` (the largest alignment
    /// of any header field) regardless of `A` — the chunk stores no
    /// allocator copy.
    #[test]
    fn struct_align_matches_usize() {
        assert_eq!(LocalChunk::<Global>::struct_align(), mem::align_of::<usize>());
    }

    /// For local chunks the value and base alignments coincide (no
    /// `CHUNK_ALIGN` over-alignment), and both equal the real
    /// `align_of_val` of a constructed chunk — so the allocation size is
    /// rounded to exactly the chunk's own alignment (never inflated).
    #[test]
    fn value_align_matches_struct_align_and_real_alignment() {
        assert_eq!(LocalChunk::<Global>::value_align(), LocalChunk::<Global>::struct_align());
        // SAFETY: single-threaded test owning the only reference.
        unsafe {
            let chunk = LocalChunk::<Global>::allocate(&Global, core::ptr::null(), 64).expect("allocate chunk");
            let real = mem::align_of_val(chunk.as_ref());
            assert_eq!(
                LocalChunk::<Global>::value_align(),
                real,
                "value_align must equal align_of_val of the real chunk DST"
            );
            LocalChunk::destroy(chunk, &Global);
        }
    }

    /// Local-chunk `chunk_layout` rounds the size up to `value_align` (8)
    /// and keeps the base alignment at `value_align` too (no
    /// over-alignment); the resulting size matches each size class exactly.
    #[test]
    fn chunk_layout_sizes_match_classes() {
        use super::super::chunk_alloc::chunk_layout;
        use super::super::constants::{NUM_CHUNK_CLASSES, SizeClass};

        let header = LocalChunk::<Global>::header_size();
        let value_align = LocalChunk::<Global>::value_align();
        let base_align = LocalChunk::<Global>::struct_align();
        for i in 0..NUM_CHUNK_CLASSES {
            let class = SizeClass::new(i);
            let total = class.bytes();
            let payload = total - header;
            let layout = chunk_layout(header, payload, value_align, base_align).expect("layout fits");
            assert_eq!(layout.size(), total, "class {i} size must equal class bytes");
            assert_eq!(layout.align(), value_align);
        }
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

    /// `header_size` is `offset_of!(<last field>) + size_of::<<last field>>()`.
    /// For `LocalChunk<Global>`, the fixed header is 28 bytes; with `stats`,
    /// appended `wasted_at_retire: Cell<u32>` makes it 32 bytes. `next`
    /// precedes the small fields so they pack without trailing padding.
    #[test]
    fn header_size_for_global_matches_layout() {
        #[cfg(not(feature = "stats"))]
        assert_eq!(LocalChunk::<Global>::header_size(), 28);
        #[cfg(feature = "stats")]
        assert_eq!(LocalChunk::<Global>::header_size(), 32);
    }

    /// `Chunk::inc_ref` on a local chunk must panic; local chunks have at most
    /// one owner and refcount bumps belong to `SharedChunk`.
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
