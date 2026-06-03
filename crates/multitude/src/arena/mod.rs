// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(clippy::inline_always, reason = "hot bump-allocator helpers must inline into their callers")]

use alloc::sync::Arc as StdArc;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::fmt;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator, Global};

use crate::arena_builder::ArenaBuilder;
#[cfg(feature = "stats")]
use crate::arena_stats::ArenaStats;
use crate::internal::chunk_mutator::ChunkMutator;
use crate::internal::chunk_provider::{ChunkProvider, ChunkProviderConfig};
use crate::internal::constants::SizeClass;
use crate::internal::current_chunk::CurrentChunk;
use crate::internal::local_chunk::LocalChunk;
use crate::internal::shared_chunk::SharedChunk;

mod alloc_growable;
pub(crate) mod alloc_prefixed;
mod alloc_slice_arc;
mod alloc_slice_box;
mod alloc_slice_ref;
mod alloc_str;
mod alloc_uninit;
#[cfg(feature = "dst")]
mod alloc_unsized;
#[cfg(feature = "utf16")]
mod alloc_utf16;
pub(crate) mod alloc_value;
mod reserve;

/// A flexible bump allocator.
///
/// Allocates large chunks of memory from an underlying allocator and parcels them out
/// efficiently in response to allocation requests.
///
/// # Configuration
///
/// [`Arena::new`] uses sensible defaults: the [`Global`] allocator,
/// no upfront preallocation, no byte budget, and a 16 KiB
/// oversized-allocation cutover (requests larger than that bypass
/// the normal chunk pool and get their own one-shot chunk sized
/// exactly to fit the request).
///
/// For non-default configuration — preallocated local/shared capacity,
/// a custom backing allocator, an outstanding-bytes budget, or a
/// different oversized-allocation cutover — use [`Arena::builder`].
///
/// # Example
///
/// ```
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let x: &mut u32 = arena.alloc(42_u32);
/// assert_eq!(*x, 42);
/// ```
pub struct Arena<A: Allocator + Clone = Global> {
    /// Currently-installed local chunk. Always populated — preloaded at
    /// construction and refilled before every successful `alloc`.
    /// [`CurrentChunk`] encapsulates the `UnsafeCell` access patterns so
    /// the hot path is a plain `self.current_local.borrow()`.
    current_local: CurrentChunk<LocalChunk<A>>,

    /// Currently-installed shared chunk. Always populated. Shared chunks
    /// produce `Arc<T>` smart pointers (never simple `&T` references), so
    /// rotation can drop the previous mutator immediately — any outstanding
    /// `Arc`s independently hold the chunk's refcount.
    current_shared: CurrentChunk<SharedChunk<A>>,

    /// Local-chunk mutators whose chunk was rotated out while it might still
    /// have outstanding simple-ref `&mut T` borrows. Each retained mutator
    /// holds a `+1` on its chunk, keeping it alive (and preventing teardown
    /// / drop-replay) until the arena is reset or dropped.
    retired_local: RefCell<Vec<ChunkMutator<LocalChunk<A>>>>,

    /// Geometric-growth chunk-class hint for the next local refill: each
    /// successful refill bumps this toward the largest cacheable class so
    /// subsequent chunks are at least as big as the previous one. Prevents
    /// the pathological "always class 0" allocation pattern that happens
    /// when small `T` types ask for tiny `worst_case_payload` sizes.
    next_local_class: Cell<SizeClass>,

    /// Same growth hint for shared chunks.
    next_shared_class: Cell<SizeClass>,

    provider: StdArc<ChunkProvider<A>>,

    /// Running counter of user-requested bytes. Updated on every
    /// successful allocation with the `Layout::size()` the caller
    /// asked for; excludes alignment padding, drop-entry overhead,
    /// and chunk headers.
    #[cfg(feature = "stats")]
    total_bytes_allocated: Cell<u64>,

    /// Running count of buffer relocations (growable collections moved to
    /// a fresh, larger buffer because they could not grow in place).
    #[cfg(feature = "stats")]
    relocations: Cell<u64>,
}

// `Arena: Send` is auto-derived: every field is `Send` (ChunkMutator
// carries its own `unsafe impl Send`, propagated through CurrentChunk's
// `UnsafeCell` and `RefCell<Vec<_>>`; `StdArc<ChunkProvider<A>>` is Send
// via `ChunkProvider`'s own `Send + Sync` impls). `Arena: !Sync` is also
// auto-derived: `CurrentChunk` and `RefCell` are both `!Sync`.

impl Arena<Global> {
    /// Create a new, empty arena backed by [`Global`] with default
    /// configuration.
    ///
    /// No chunk is allocated up front: the first allocation lazily
    /// pulls in a chunk on the slow path.
    ///
    /// For non-default configuration, use [`Self::builder`].
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let x: &mut u32 = arena.alloc(42_u32);
    /// assert_eq!(*x, 42);
    /// ```
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self::new_in(Global)
    }

    /// Fallible variant of [`Self::new`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails while
    /// preallocating the initial local and shared chunks.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // `Default::default()` mutation is observationally equivalent
    pub fn try_new() -> Result<Self, AllocError> {
        Self::try_new_in(Global)
    }

    /// Create an [`ArenaBuilder`](crate::ArenaBuilder) using [`Global`]
    /// as the backing allocator.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // `Default::default()` mutation is observationally equivalent
    pub fn builder() -> ArenaBuilder<Global> {
        ArenaBuilder::new()
    }
}

impl Default for Arena<Global> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Create an [`ArenaBuilder`](crate::ArenaBuilder) backed by a custom
    /// `allocator`.
    #[must_use]
    #[inline]
    pub fn builder_in(allocator: A) -> ArenaBuilder<A> {
        ArenaBuilder::new_in(allocator)
    }

    /// Create a new, empty arena backed by `allocator` with default
    /// configuration.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails while preallocating the
    /// initial local and shared chunks.
    #[must_use]
    #[inline]
    pub fn new_in(allocator: A) -> Self
    where
        A: 'static,
    {
        expect_alloc(Self::try_from_config(allocator, crate::internal::constants::MAX_NORMAL_ALLOC, None))
    }

    /// Fallible variant of [`Self::new_in`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails while
    /// preallocating the initial local and shared chunks.
    #[inline]
    pub fn try_new_in(allocator: A) -> Result<Self, AllocError>
    where
        A: 'static,
    {
        Self::try_from_config(allocator, crate::internal::constants::MAX_NORMAL_ALLOC, None)
    }

    /// Internal builder entry point: assemble an `Arena` from a fully
    /// resolved configuration. Construction is lazy: the current local
    /// and shared mutators start empty and the first allocation pulls
    /// a real chunk via [`Self::refill_local`] / [`Self::refill_shared`].
    #[allow(
        clippy::unnecessary_wraps,
        reason = "Result return is part of try_from_config's contract; callers propagate the error"
    )]
    pub(crate) fn try_from_config(allocator: A, max_normal_alloc: usize, byte_budget: Option<usize>) -> Result<Self, AllocError> {
        let config = ChunkProviderConfig {
            byte_budget: byte_budget.unwrap_or(usize::MAX),
            max_normal_alloc,
        };
        let provider = ChunkProvider::new(allocator, config);
        Ok(Self {
            current_local: CurrentChunk::new(ChunkMutator::<LocalChunk<A>>::empty()),
            current_shared: CurrentChunk::new(ChunkMutator::<SharedChunk<A>>::empty()),
            retired_local: RefCell::new(Vec::new()),
            next_local_class: Cell::new(SizeClass::ZERO),
            next_shared_class: Cell::new(SizeClass::ZERO),
            provider,
            #[cfg(feature = "stats")]
            total_bytes_allocated: Cell::new(0),
            #[cfg(feature = "stats")]
            relocations: Cell::new(0),
        })
    }

    /// Pre-warm the local chunk cache with one chunk in the given size
    /// class. Used by `ArenaBuilder::with_capacity_local`. Also raises
    /// `next_local_class` so the first refill consumes the cached chunk.
    #[cfg_attr(test, mutants::skip)] // `>` vs `>=` is an identity-write difference
    pub(crate) fn preallocate_one_local(&self, class: SizeClass) -> Result<(), AllocError> {
        self.provider.preallocate_local(class)?;
        if class > self.next_local_class.get() {
            self.next_local_class.set(class);
        }
        Ok(())
    }

    /// Pre-warm the shared chunk cache with one chunk in the given size
    /// class. Used by `ArenaBuilder::with_capacity_shared`. Also raises
    /// `next_shared_class` so the first refill consumes the cached chunk.
    #[cfg_attr(test, mutants::skip)] // see `preallocate_one_local`
    pub(crate) fn preallocate_one_shared(&self, class: SizeClass) -> Result<(), AllocError> {
        self.provider.preallocate_shared(class)?;
        if class > self.next_shared_class.get() {
            self.next_shared_class.set(class);
        }
        Ok(())
    }

    /// Borrow the backing allocator.
    #[must_use]
    #[inline]
    pub fn allocator(&self) -> &A {
        self.provider.allocator()
    }

    /// Snapshot of the arena's lifetime statistics.
    #[cfg(feature = "stats")]
    #[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
    #[must_use]
    #[inline]
    pub fn stats(&self) -> ArenaStats {
        let chunks = self.provider.chunk_alloc_stats();
        ArenaStats {
            total_bytes_allocated: self.total_bytes_allocated.get(),
            normal_local_chunks_allocated: chunks.normal_local,
            oversized_local_chunks_allocated: chunks.oversized_local,
            normal_shared_chunks_allocated: chunks.normal_shared,
            oversized_shared_chunks_allocated: chunks.oversized_shared,
            relocations: self.relocations.get(),
            ..ArenaStats::default()
        }
    }

    /// Record a successful user allocation of `bytes` bytes.
    #[cfg(feature = "stats")]
    #[inline(always)]
    pub(crate) fn record_alloc(&self, bytes: usize) {
        let prev = self.total_bytes_allocated.get();
        self.total_bytes_allocated.set(prev + bytes as u64);
    }

    /// Record a buffer relocation (a growable collection moved to a fresh
    /// allocation because it could not grow in place).
    #[cfg(feature = "stats")]
    #[inline(always)]
    pub(crate) fn record_relocation(&self) {
        self.relocations.set(self.relocations.get() + 1);
    }

    /// Reset the arena to a fresh state, ready for a new allocation phase.
    ///
    /// Given that this takes `&mut self`, the borrow checker ensures no
    /// outstanding simple references can still be live. Outstanding `Arc`s
    /// from shared chunks continue to hold their backing chunks alive
    /// independently.
    ///
    /// The reset is lazy: the current chunk slots are returned to the
    /// empty state and a fresh chunk is acquired on the first subsequent
    /// allocation, mirroring the lazy semantics of [`Self::new`].
    #[cold]
    pub fn reset(&mut self) {
        self.retired_local.borrow_mut().clear();
        *self.current_local.get_mut() = ChunkMutator::<LocalChunk<A>>::empty();
        *self.current_shared.get_mut() = ChunkMutator::<SharedChunk<A>>::empty();
    }

    /// Returns a [`ZerocopyView`](crate::zerocopy::ZerocopyView)
    /// providing safe zero-initialized allocation for types implementing
    /// [`zerocopy::FromZeros`](::zerocopy::FromZeros).
    #[cfg(feature = "zerocopy")]
    #[cfg_attr(docsrs, doc(cfg(feature = "zerocopy")))]
    #[inline]
    #[must_use]
    pub const fn zerocopy(&self) -> crate::zerocopy::ZerocopyView<'_, A> {
        crate::zerocopy::ZerocopyView::new(self)
    }

    /// Returns a [`BytemuckView`](crate::bytemuck::BytemuckView)
    /// providing safe zero-initialized allocation for types implementing
    /// [`bytemuck::Zeroable`](::bytemuck::Zeroable).
    #[cfg(feature = "bytemuck")]
    #[cfg_attr(docsrs, doc(cfg(feature = "bytemuck")))]
    #[inline]
    #[must_use]
    pub const fn bytemuck(&self) -> crate::bytemuck::BytemuckView<'_, A> {
        crate::bytemuck::BytemuckView::new(self)
    }

    // ----- internal: refill helpers -----

    /// Borrow the current local mutator for a single bump attempt. Used
    /// by the hot path of every local-chunk allocator.
    ///
    /// The returned reference is valid only until the next
    /// [`Self::refill_local`] call; see [`CurrentChunk`]'s soundness
    /// contract for the in-module aliasing rule.
    #[inline(always)]
    pub(crate) fn current_local(&self) -> &ChunkMutator<LocalChunk<A>> {
        self.current_local.borrow()
    }

    /// Largest single allocation routed through normal size classes.
    /// Requests above this are served by one-shot oversized chunks.
    #[inline]
    pub(crate) fn max_normal_alloc(&self) -> usize {
        self.provider.config().max_normal_alloc
    }

    /// True iff a shared-chunk allocation request of `min_payload` bytes
    /// must be routed to a one-shot oversized chunk instead of the normal
    /// size-class pool. Callers that detect this case should use
    /// [`Self::alloc_oversized_shared_with`] rather than
    /// [`Self::refill_shared`].
    ///
    /// `ArenaBuilder` caps `max_normal_alloc` at `max_bump_extent`
    /// (`MAX_CHUNK_BYTES - header_size`), so `min_payload <=
    /// max_normal_alloc` always implies `header + min_payload <=
    /// MAX_CHUNK_BYTES` — a single threshold check is enough.
    #[inline]
    pub(crate) fn is_oversized_shared(&self, min_payload: usize) -> bool {
        min_payload > self.max_normal_alloc()
    }

    /// Local mirror of [`Self::is_oversized_shared`].
    #[inline]
    pub(crate) fn is_oversized_local(&self, min_payload: usize) -> bool {
        min_payload > self.max_normal_alloc()
    }

    /// Attempt to grow a buffer in place within the current local chunk.
    ///
    /// Succeeds only when the buffer's storage (`[base_addr, base_addr +
    /// old_bytes)`) ends exactly at the chunk's bump cursor and the chunk
    /// has room to extend it to `new_bytes`. On success the bump cursor is
    /// advanced and no relocation/copy occurs.
    #[inline]
    pub(crate) fn try_grow_local_in_place(&self, base_addr: usize, old_bytes: usize, new_bytes: usize) -> bool {
        self.current_local.borrow().try_grow_in_place(base_addr, old_bytes, new_bytes)
    }

    /// Borrow the current shared mutator for a single bump attempt.
    ///
    /// Same lifetime contract as [`Self::current_local`].
    #[inline(always)]
    pub(crate) fn current_shared(&self) -> &ChunkMutator<SharedChunk<A>> {
        self.current_shared.borrow()
    }

    /// Retire the current local mutator into `retired_local` and install a
    /// fresh chunk that satisfies `min_payload` bytes.
    #[cold]
    #[inline(never)]
    // Mutation testing is suppressed: body→`Ok(())` makes refill a
    // no-op while callers continue to fail `try_alloc` and re-enter
    // here, producing an infinite loop the timeout traps.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn refill_local(&self, min_payload: usize) -> Result<(), AllocError> {
        let new_chunk = self.provider.acquire_local(min_payload, self.next_local_class.get())?;
        // SAFETY: `acquire_local` returns a refcount-1 chunk; the +1 is
        // moved into the new ChunkMutator.
        let new_mutator = unsafe { ChunkMutator::<LocalChunk<A>>::from_owned(new_chunk) };
        let old = self.current_local.replace(new_mutator);
        self.retired_local.borrow_mut().push(old);
        self.next_local_class.set(self.next_local_class.get().saturating_inc());
        Ok(())
    }

    /// Replace the current shared mutator with a fresh chunk that satisfies
    /// `min_payload` bytes. The previous mutator is dropped immediately —
    /// any outstanding `Arc`s independently keep the prior chunk alive.
    ///
    /// The caller must have verified `!self.is_oversized_shared(min_payload)`
    /// before invoking this; oversized requests must go through
    /// [`Self::alloc_oversized_shared_with`] so they don't replace (and
    /// thus waste) the current chunk.
    #[cold]
    #[inline(never)]
    #[cfg_attr(test, mutants::skip)] // see `refill_local`
    pub(crate) fn refill_shared(&self, min_payload: usize) -> Result<(), AllocError> {
        // Release the exhausted current chunk's refcount *before* reserving
        // the replacement so a now-unreferenced chunk frees its bytes and
        // lets the new reservation reuse the budget.
        self.current_shared.drop_replace(ChunkMutator::<SharedChunk<A>>::empty());
        let new_chunk = self.provider.acquire_shared(min_payload, self.next_shared_class.get())?;
        // SAFETY: `acquire_shared` returns a refcount-1 chunk.
        let new_mutator = unsafe { ChunkMutator::<SharedChunk<A>>::from_owned(new_chunk) };
        self.current_shared.drop_replace(new_mutator);
        self.next_shared_class.set(self.next_shared_class.get().saturating_inc());
        Ok(())
    }

    /// Acquires a one-shot oversized **shared** chunk sized to fit
    /// `min_payload` bytes, builds a temporary [`ChunkMutator`] over it
    /// on the stack, and invokes `do_alloc` to perform the single
    /// allocation. The arena's current shared mutator is **not**
    /// touched, so subsequent small allocations continue to use the
    /// existing chunk.
    ///
    /// The temporary mutator is dropped before this function returns:
    /// it publishes its drop-entry count and releases its own `+1`
    /// strong reference. If `do_alloc` retained a `+1` on the chunk
    /// (the smart-pointer case), the chunk stays alive via that ref;
    /// otherwise (e.g. an init panic before the `+1` was taken) the
    /// chunk is torn down here.
    #[cold]
    #[inline(never)]
    pub(crate) fn alloc_oversized_shared_with<R>(
        &self,
        min_payload: usize,
        do_alloc: impl FnOnce(&ChunkMutator<SharedChunk<A>>, NonNull<SharedChunk<A>>) -> R,
    ) -> Result<R, AllocError> {
        let chunk = self.provider.acquire_oversized_shared(min_payload)?;
        // SAFETY: `acquire_oversized_shared` returns a refcount-1 chunk;
        // the `+1` moves into the temporary mutator.
        let mutator = unsafe { ChunkMutator::<SharedChunk<A>>::from_owned(chunk) };
        Ok(do_alloc(&mutator, chunk))
    }

    /// Local mirror of [`Self::alloc_oversized_shared_with`]. The
    /// temporary mutator is pushed into `retired_local` on success so
    /// the chunk's `+1` strong reference is retained for the duration
    /// of the `&Arena` borrow — required for `&mut T` / `&mut [T]` /
    /// `&mut str` allocations that have no independent refcount.
    ///
    /// If `do_alloc` panics, the mutator is dropped on unwind (its `+1`
    /// is released), and the chunk is torn down before the panic
    /// propagates.
    #[cold]
    #[inline(never)]
    pub(crate) fn alloc_oversized_local_with<R>(
        &self,
        min_payload: usize,
        do_alloc: impl FnOnce(&ChunkMutator<LocalChunk<A>>) -> R,
    ) -> Result<R, AllocError> {
        let chunk = self.provider.acquire_oversized_local(min_payload)?;
        // SAFETY: `acquire_oversized_local` returns a refcount-1 chunk.
        let mutator = unsafe { ChunkMutator::<LocalChunk<A>>::from_owned(chunk) };
        let result = do_alloc(&mutator);
        // Retain the mutator (and its `+1`) for the `&Arena` lifetime.
        self.retired_local.borrow_mut().push(mutator);
        Ok(result)
    }
}

impl<A: Allocator + Clone> fmt::Debug for Arena<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Arena").finish_non_exhaustive()
    }
}

// No explicit `Drop` impl: field drops (Cells/RefCells of mutators) release
// chunk refcounts, and the `Arc<ChunkProvider>` releases the cache, which
// returns retained chunks to the backing allocator.

/// Convert a fallible alloc result to its `Ok` value, panicking on
/// `Err` with the canonical multitude allocator-failure message.
///
/// Compared to a bare `(…).expect_alloc()`,
/// the call site here is a regular method-call expression that LLVM does
/// not see as diverging — so line-coverage tracks each caller of this
/// helper without leaving its `Err`-arm uncovered.
pub(crate) trait ExpectAlloc<T> {
    fn expect_alloc(self) -> T;
}

impl<T> ExpectAlloc<T> for Result<T, AllocError> {
    #[inline]
    #[track_caller]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn expect_alloc(self) -> T {
        #[allow(clippy::panic, reason = "documented panic path of the panicking alloc API")]
        #[allow(clippy::match_wild_err_arm, reason = "documented panic path of the panicking alloc API")]
        match self {
            Ok(v) => v,
            Err(_) => panic!("multitude: allocator returned AllocError"),
        }
    }
}

/// Cold panicking helper used by the panicking allocator variants.
///
/// Implemented as a macro that expands to an `ExpectAlloc::expect_alloc`
/// call on a pre-failed `Result`. The method is **not** a `-> !`
/// function from LLVM's point of view (the trait method itself returns
/// `T`), so the call site stays a regular function-call expression and
/// `llvm-cov` is able to count the surrounding line. The divergence
/// happens inside `expect_alloc`'s body, which is marked
/// `#[cfg_attr(coverage_nightly, coverage(off))]`.
macro_rules! panic_alloc {
    () => {{
        $crate::arena::ExpectAlloc::expect_alloc(::core::result::Result::<(), allocator_api2::alloc::AllocError>::Err(
            allocator_api2::alloc::AllocError,
        ))
    }};
}
pub(crate) use panic_alloc;

/// Pointer wrapper that converts an [`AllocError`] to a panic. Used to
/// wrap fallible internal alloc paths with panicking facades.
#[cold]
#[inline(never)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) fn expect_alloc<T>(r: Result<T, AllocError>) -> T {
    (r).expect_alloc()
}
