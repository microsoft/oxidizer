// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::{Allocator, Global};

use crate::blind_pool::{BlindPool, ChunkSizing};

/// Default per-chunk byte target.
///
/// Chosen so that a chunk of small values costs about one page while a chunk of
/// kilobyte-scale values still holds several slots.
const DEFAULT_CHUNK_BYTES: usize = 4096;

/// Configures and builds a [`BlindPool`].
///
/// ```
/// let pool = plurality::BlindPool::builder()
///     .chunk_bytes(8192)
///     .max_layouts(16)
///     .build();
/// assert_eq!(pool.max_layouts(), Some(16));
/// ```
#[derive(Debug)]
pub struct BlindPoolBuilder<A: Allocator + Clone = Global> {
    sizing: ChunkSizing,
    max_chunks: Option<u32>,
    max_layouts: Option<usize>,
    allocator: A,
}

impl BlindPoolBuilder<Global> {
    /// Creates a builder with the default byte target, unbounded growth, and
    /// the global allocator.
    ///
    /// Crate-internal: the public entry point is
    /// [`BlindPool::builder`](crate::BlindPool::builder), per the builder
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

impl<A: Allocator + Clone> BlindPoolBuilder<A> {
    /// Sizes chunks by a byte target, so that layouts of very different sizes
    /// commit comparable memory per growth step.
    ///
    /// Each layout pool derives its own slot count by dividing the target by
    /// its slot stride, rounded down to a power of two and clamped to at least
    /// one slot. Replaces any previous [`chunk_size`](Self::chunk_size).
    #[must_use]
    pub fn chunk_bytes(mut self, bytes: usize) -> Self {
        self.sizing = ChunkSizing::Bytes(bytes);
        self
    }

    /// Sizes chunks by a slot count, reproducing the typed pool's
    /// predictability.
    ///
    /// Every layout starts from this count, subject to per-layout clamping.
    /// Replaces any previous [`chunk_bytes`](Self::chunk_bytes).
    #[must_use]
    pub fn chunk_size(mut self, slots_per_chunk: u32) -> Self {
        self.sizing = ChunkSizing::Slots(slots_per_chunk);
        self
    }

    /// Caps the number of chunks **per layout**. Omit for unbounded growth.
    ///
    /// The effective cap for a layout is the smaller of this and the ceiling
    /// its chunk size permits; read it back with
    /// [`BlindPool::max_chunks_of`](crate::BlindPool::max_chunks_of).
    #[must_use]
    pub fn max_chunks(mut self, max: u32) -> Self {
        self.max_chunks = Some(max);
        self
    }

    /// Caps the number of distinct layouts. Omit for unbounded growth.
    ///
    /// Once reached, allocating a value of an unseen layout reports capacity
    /// exhaustion rather than creating a pool for it.
    #[must_use]
    pub fn max_layouts(mut self, max: usize) -> Self {
        self.max_layouts = Some(max);
        self
    }

    /// Swaps in a custom allocator for chunk allocations.
    ///
    /// Each layout pool owns its own clone, so the allocator must be
    /// cloneable.
    #[must_use]
    pub fn allocator<A2: Allocator + Clone>(self, allocator: A2) -> BlindPoolBuilder<A2> {
        BlindPoolBuilder {
            sizing: self.sizing,
            max_chunks: self.max_chunks,
            max_layouts: self.max_layouts,
            allocator,
        }
    }

    /// Builds the pool.
    #[must_use]
    #[cold]
    pub fn build(self) -> BlindPool<A> {
        BlindPool::from_parts(self.sizing, self.max_chunks, self.max_layouts, self.allocator)
    }
}
