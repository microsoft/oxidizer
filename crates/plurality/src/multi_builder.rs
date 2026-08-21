// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::{Allocator, Global};

use crate::multi_pool::{ChunkSizing, MultiPool};
use crate::slot::MAX_CHUNK_SIZE_SLOTS;

/// Default per-chunk byte target.
///
/// Chosen so that a chunk of small values costs about one page while a chunk of
/// kilobyte-scale values still holds several slots.
const DEFAULT_CHUNK_BYTES: usize = 4096;

/// Configures and builds a [`MultiPool`].
///
/// ```
/// let pool = plurality::MultiPool::builder()
///     .chunk_bytes(8192)
///     .max_layouts(16)
///     .build();
/// assert_eq!(pool.max_layouts(), Some(16));
/// ```
#[derive(Debug)]
pub struct MultiPoolBuilder<A: Allocator + Clone = Global> {
    sizing: ChunkSizing,
    max_chunks: Option<u32>,
    max_layouts: Option<usize>,
    allocator: A,
}

impl MultiPoolBuilder<Global> {
    /// Creates a builder with the default byte target, unbounded growth, and
    /// the global allocator.
    ///
    /// Crate-internal: the public entry point is
    /// [`MultiPool::builder`](crate::MultiPool::builder), per the builder
    /// convention that a builder is obtained from its target type.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            sizing: ChunkSizing::Bytes(DEFAULT_CHUNK_BYTES),
            max_chunks: None,
            max_layouts: None,
            allocator: Global,
        }
    }
}

impl<A: Allocator + Clone> MultiPoolBuilder<A> {
    /// Sizes chunks by a byte target, so that layouts of very different sizes
    /// commit comparable memory per growth step.
    ///
    /// Each layout pool derives its own slot count by dividing the target by
    /// its slot stride, rounded down to a power of two and clamped to at least
    /// one slot. The builder uses whichever sizing method appears last in the
    /// chain.
    #[must_use]
    pub fn chunk_bytes(mut self, bytes: usize) -> Self {
        self.sizing = ChunkSizing::Bytes(bytes);
        self
    }

    /// Sizes chunks from a requested slot count.
    ///
    /// The request is bounded and rounded up to the next power of two before a
    /// layout pool reduces it for chunk-layout overflow. Each layout starts
    /// from that normalized count, so read
    /// [`MultiPool::chunk_size_of`](crate::MultiPool::chunk_size_of) for the
    /// effective per-layout value. The builder uses whichever sizing method
    /// appears last in the chain.
    #[must_use]
    pub fn chunk_size(mut self, slots_per_chunk: u32) -> Self {
        self.sizing = ChunkSizing::Slots(slots_per_chunk);
        self
    }

    /// Caps the number of chunks **per internal layout pool**. Omit for
    /// unbounded growth.
    ///
    /// The effective cap for a layout is the smaller of this and the ceiling
    /// its chunk size permits; read it back with
    /// [`MultiPool::max_chunks_of`](crate::MultiPool::max_chunks_of).
    #[must_use]
    pub fn max_chunks(mut self, max: u32) -> Self {
        self.max_chunks = Some(max);
        self
    }

    /// Caps the number of internal layout pools. Omit for unbounded growth.
    ///
    /// Once reached, allocating a value that would need a new layout pool
    /// reports capacity exhaustion rather than creating one.
    #[must_use]
    pub fn max_layouts(mut self, max: usize) -> Self {
        self.max_layouts = Some(max);
        self
    }

    /// Swaps in a custom allocator for chunk allocations.
    ///
    /// Each layout pool owns its own clone, so the allocator must be
    /// cloneable.
    ///
    /// The allocator may allocate from, and free into, the pool it serves. An
    /// allocator that does so unconditionally recurses until the stack is
    /// exhausted, since serving the nested allocation calls it again.
    #[must_use]
    pub fn allocator<A2: Allocator + Clone>(self, allocator: A2) -> MultiPoolBuilder<A2> {
        MultiPoolBuilder {
            sizing: self.sizing,
            max_chunks: self.max_chunks,
            max_layouts: self.max_layouts,
            allocator,
        }
    }

    /// Builds the pool.
    ///
    /// # Panics
    ///
    /// Panics if the requested `chunk_bytes` or `chunk_size` is `0`, or if
    /// `chunk_size` is greater than `2^31` (the largest `u32` whose next power
    /// of two is representable).
    #[must_use]
    #[cold]
    pub fn build(self) -> MultiPool<A> {
        // The caller-supplied request is validated here; the per-layout
        // reduction each layout pool then applies is a property of the layout,
        // not of the request. Ref: docs/design/multi-pool.md, "Bounding growth".
        match self.sizing {
            ChunkSizing::Bytes(bytes) => assert!(bytes >= 1, "chunk_bytes must be >= 1"),
            ChunkSizing::Slots(slots) => {
                assert!(slots >= 1, "chunk_size must be >= 1");
                assert!(
                    slots <= MAX_CHUNK_SIZE_SLOTS,
                    "chunk_size exceeds the largest slot count with a representable next power of two"
                );
            }
        }
        MultiPool::from_parts(self.sizing, self.max_chunks, self.max_layouts, self.allocator)
    }
}
