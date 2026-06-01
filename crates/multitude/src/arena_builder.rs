// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;
use core::marker::PhantomData;

use allocator_api2::alloc::{AllocError, Allocator, Global};

use crate::Arena;
use crate::internal::constants::{
    MAX_NORMAL_ALLOC, MIN_CHUNK_BYTES, MIN_MAX_NORMAL_ALLOC, NUM_CHUNK_CLASSES, class_to_bytes, min_class_for_bytes,
};

/// Fluent builder for [`Arena`].
///
/// All knobs have sensible defaults. The defaults reproduce
/// `Arena::new()` exactly.
pub struct ArenaBuilder<A: Allocator + Clone = Global> {
    allocator: A,
    max_normal_alloc: usize,
    byte_budget: Option<usize>,
    /// Bytes the local cache should hold up front. `0` means none.
    /// Must be either `0` or `>= MIN_CHUNK_BYTES` (512). See
    /// [`Self::with_capacity_local`].
    capacity_local: usize,
    /// Bytes the shared cache should hold up front. `0` means none.
    /// Must be either `0` or `>= MIN_CHUNK_BYTES` (512). See
    /// [`Self::with_capacity_shared`].
    capacity_shared: usize,
    _phantom: PhantomData<A>,
}

impl ArenaBuilder<Global> {
    /// Start a new builder with default knobs and the [`Global`] allocator.
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self::new_in(Global)
    }
}

impl Default for ArenaBuilder<Global> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Allocator + Clone> ArenaBuilder<A> {
    /// Start a new builder with default knobs and a custom backing
    /// allocator.
    #[must_use]
    #[inline]
    pub fn new_in(allocator: A) -> Self {
        Self {
            allocator,
            max_normal_alloc: MAX_NORMAL_ALLOC,
            byte_budget: None,
            capacity_local: 0,
            capacity_shared: 0,
            _phantom: PhantomData,
        }
    }

    /// Set the size threshold above which a request that needs a fresh
    /// chunk gets its own oversized chunk.
    ///
    /// This threshold only governs **chunk acquisition**, not the bump
    /// fast path: an allocation strictly larger than `max_normal_alloc`
    /// that happens to fit in the tail of the chunk already installed
    /// as `current_{local,shared}` is satisfied from that chunk (the
    /// goal here is to avoid wasting bump space when the caller paid
    /// for it via `with_capacity_*` or the high-water ratchet). It is
    /// only when no current chunk can satisfy the request — i.e. when
    /// we'd otherwise call `refill_local`/`refill_shared` — that we
    /// route oversized requests to a dedicated one-shot chunk that is
    /// never cached.
    ///
    /// Must be in `[4096, MAX_CHUNK_BYTES - chunk_header_size]`. The
    /// lower bound is fixed; the upper bound is approximately 64 KiB
    /// but is reduced by the per-chunk header size, which depends on
    /// the backing allocator type `A`. Out-of-range values cause
    /// [`Self::build`] / [`Self::try_build`] to panic with the
    /// resolved bounds in the panic message.
    #[must_use]
    #[inline]
    pub const fn max_normal_alloc(mut self, bytes: usize) -> Self {
        self.max_normal_alloc = bytes;
        self
    }

    /// Set a cap on the total bytes of chunk capacity that may be
    /// outstanding at any one time (live + cached).
    ///
    /// The counter goes up on every fresh chunk allocation and down
    /// on every chunk free, so cached chunks count against the
    /// budget and released chunks free their share. When a fresh
    /// allocation would push the counter past the budget,
    /// [`AllocError`] is returned instead — this is not a
    /// lifetime-cumulative limit.
    ///
    /// Counting convention: each chunk consumes its **total
    /// allocation size** (`class_to_bytes(class)` for cached chunks;
    /// `header_size + round_payload(user_request)` for one-shot
    /// oversized chunks) from the budget. Under the post-resize
    /// chunk layout this matches the underlying VM allocation
    /// exactly (up to a small structural-alignment rounding for
    /// oversized shared chunks, at most 63 bytes).
    #[must_use]
    #[inline]
    pub const fn byte_budget(mut self, bytes: usize) -> Self {
        self.byte_budget = Some(bytes);
        self
    }

    /// Preallocate `bytes` bytes of **total local chunk allocation**
    /// up front (header + payload). Allows the arena to handle a
    /// burst of local-flavor (`alloc`, `alloc_rc`, `alloc_box`,
    /// builders) allocations without growing the local chunk pool.
    ///
    /// Concretely, the builder picks the smallest size class whose
    /// total allocation is at least `bytes`, capped at the largest
    /// class (64 KiB). It then allocates enough chunks of that class
    /// to cover `bytes` and pushes them all into the local chunk
    /// cache. The local high-water mark is also seeded to this
    /// class so the very first local allocation already runs
    /// against a chunk of the workload's natural size.
    ///
    /// `bytes` is total chunk allocation, not user-visible payload
    /// (the chunk header costs a few dozen bytes per chunk, so the
    /// payload available to allocations is slightly smaller).
    /// `bytes` must be `0` (no preallocation; the default) or at
    /// least 512 (the smallest chunk class). Out-of-range values
    /// cause [`Self::build`] / [`Self::try_build`] to panic.
    #[must_use]
    #[inline]
    pub const fn with_capacity_local(mut self, bytes: usize) -> Self {
        self.capacity_local = bytes;
        self
    }

    /// Preallocate `bytes` bytes of **total shared chunk allocation**
    /// up front (header + payload). Allows the arena to handle a
    /// burst of shared-flavor (`alloc_arc`, `try_alloc_arc`, etc.)
    /// allocations without growing the shared chunk pool.
    ///
    /// Mirror of [`Self::with_capacity_local`] for the shared cache:
    /// the builder picks the smallest class whose total covers
    /// `bytes`, allocates enough chunks of that class to cover
    /// `bytes`, pushes them onto the shared cache Treiber stack,
    /// and seeds the shared high-water mark to the appropriate class.
    /// `bytes` is total chunk allocation, not user-visible payload.
    /// `bytes` must be `0` (no preallocation; the default) or at
    /// least 512.
    #[must_use]
    #[inline]
    pub const fn with_capacity_shared(mut self, bytes: usize) -> Self {
        self.capacity_shared = bytes;
        self
    }

    /// Replace the backing allocator. Returns a builder over the new
    /// allocator type with all other settings preserved.
    #[must_use]
    #[inline]
    pub fn allocator_in<A2: Allocator + Clone>(self, allocator: A2) -> ArenaBuilder<A2> {
        ArenaBuilder {
            allocator,
            max_normal_alloc: self.max_normal_alloc,
            byte_budget: self.byte_budget,
            capacity_local: self.capacity_local,
            capacity_shared: self.capacity_shared,
            _phantom: PhantomData,
        }
    }

    /// Validate this builder's configuration. Panics if any knob is
    /// out of range.
    #[cold]
    fn validate(&self) {
        // `max_normal_alloc` must fit in both local and shared cached
        // chunks, so cap it at the smaller max bump extent.
        let upper = crate::internal::local_chunk::max_bump_extent::<A>().min(crate::internal::shared_chunk::max_bump_extent::<A>());
        assert!(
            (MIN_MAX_NORMAL_ALLOC..=upper).contains(&self.max_normal_alloc),
            "max_normal_alloc must be in [{MIN_MAX_NORMAL_ALLOC}, {upper}], got {}",
            self.max_normal_alloc,
        );
        assert!(
            self.capacity_local == 0 || self.capacity_local >= MIN_CHUNK_BYTES,
            "with_capacity_local(bytes) must be either 0 or at least {MIN_CHUNK_BYTES}, got {}",
            self.capacity_local,
        );
        assert!(
            self.capacity_shared == 0 || self.capacity_shared >= MIN_CHUNK_BYTES,
            "with_capacity_shared(bytes) must be either 0 or at least {MIN_CHUNK_BYTES}, got {}",
            self.capacity_shared,
        );
    }

    /// Resolve a desired preallocation `capacity` (total
    /// chunk-allocation bytes) into a `(target_class, chunk_count)`
    /// pair: smallest class whose `class_to_bytes(c) >= capacity`
    /// (saturated at `NUM_CHUNK_CLASSES - 1`), times enough chunks
    /// to cover `capacity`.
    #[cfg_attr(test, mutants::skip)] // Chunk-class clamp mutations still choose a class that satisfies the request.
    fn resolve_capacity(capacity: usize) -> Option<(u8, usize)> {
        if capacity == 0 {
            return None;
        }
        let target_class = min_class_for_bytes(capacity).min(NUM_CHUNK_CLASSES - 1);
        let class_total = class_to_bytes(target_class);
        let count = capacity.div_ceil(class_total);
        Some((target_class, count))
    }

    /// Consume this builder and produce a configured [`Arena`].
    ///
    /// # Panics
    ///
    /// Panics if any builder knob is out of range, or if the backing
    /// allocator fails while preallocating chunks.
    #[must_use]
    #[cold]
    pub fn build(self) -> Arena<A>
    where
        A: 'static,
    {
        match self.try_build() {
            Ok(a) => a,
            Err(_) => panic_build(),
        }
    }

    /// Fallible variant of [`Self::build`].
    ///
    /// # Panics
    ///
    /// Panics if any builder knob is out of range. Allocator failures
    /// (e.g. during preallocation) are returned as [`AllocError`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails while
    /// preallocating chunks.
    #[cold]
    pub fn try_build(self) -> Result<Arena<A>, AllocError>
    where
        A: 'static,
    {
        self.validate();
        let local = Self::resolve_capacity(self.capacity_local);
        let shared = Self::resolve_capacity(self.capacity_shared);
        let initial_local_class = local.map_or(0, |(c, _)| c);
        let initial_shared_class = shared.map_or(0, |(c, _)| c);
        let arena = Arena::from_config(
            self.allocator,
            self.max_normal_alloc,
            self.byte_budget,
            initial_local_class,
            initial_shared_class,
        );
        if let Some((_, n)) = local {
            for _ in 0..n {
                arena.preallocate_one_local()?;
            }
        }
        if let Some((_, n)) = shared {
            for _ in 0..n {
                arena.preallocate_one_shared()?;
            }
        }
        Ok(arena)
    }
}

#[expect(
    clippy::missing_fields_in_debug,
    reason = "Allocator and PhantomData fields are not useful in debug output"
)]
impl<A: Allocator + Clone> fmt::Debug for ArenaBuilder<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArenaBuilder")
            .field("max_normal_alloc", &self.max_normal_alloc)
            .field("byte_budget", &self.byte_budget)
            .field("capacity_local", &self.capacity_local)
            .field("capacity_shared", &self.capacity_shared)
            .finish()
    }
}

#[cold]
#[inline(never)]
#[expect(clippy::panic, reason = "panicking constructor matches Arena's `panic_alloc` style")]
fn panic_build() -> ! {
    panic!("multitude::ArenaBuilder::build: backing allocator failed");
}
