// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box as AllocBox;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ptr::NonNull;

use allocator_api2::alloc::{Allocator, Global};

use crate::atomic::{AtomicU32, AtomicUsize};
use crate::chunk::chunk_layout;
use crate::pool::{Pool, PoolCore, PoolInner, teardown_erased};
use crate::slot::{FREE_END, MAX_POOL_SLOTS};

/// Default number of slots per chunk.
const DEFAULT_CHUNK_SIZE: u32 = 32;

/// Configures and builds a [`Pool`].
///
/// ```
/// let pool = plurality::Pool::<u64>::builder()
///     .chunk_size(64)
///     .max_chunks(16)
///     .build();
/// assert_eq!(pool.chunk_size(), 64);
/// assert_eq!(pool.max_capacity(), Some(64 * 16));
/// ```
#[derive(Debug)]
pub struct PoolBuilder<T, A: Allocator = Global> {
    chunk_size: u32,
    max_chunks: Option<u32>,
    allocator: A,
    _marker: PhantomData<fn() -> T>,
}

impl<T> PoolBuilder<T, Global> {
    /// Creates a builder with the default chunk size, unbounded growth, and the
    /// global allocator.
    ///
    /// Crate-internal: the public entry point is [`Pool::builder`](crate::Pool::builder),
    /// per the builder convention that a builder is obtained from its target
    /// type, not constructed directly.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            max_chunks: None,
            allocator: Global,
            _marker: PhantomData,
        }
    }
}

impl<T, A: Allocator> PoolBuilder<T, A> {
    /// Sets the requested number of slots per chunk.
    ///
    /// The value is rounded up to a power of two (so index math is a shift/mask)
    /// when the pool is [built](Self::build); the effective size is then
    /// available via [`Pool::chunk_size`](crate::Pool::chunk_size).
    #[must_use]
    pub fn chunk_size(mut self, slots_per_chunk: u32) -> Self {
        self.chunk_size = slots_per_chunk;
        self
    }

    /// Caps the number of chunks (so the maximum capacity is
    /// `chunk_size * max`). Omit for unbounded growth.
    #[must_use]
    pub fn max_chunks(mut self, max: u32) -> Self {
        self.max_chunks = Some(max);
        self
    }

    /// Swaps in a custom allocator for chunk allocations.
    #[must_use]
    pub fn allocator<A2: Allocator>(self, allocator: A2) -> PoolBuilder<T, A2> {
        PoolBuilder {
            chunk_size: self.chunk_size,
            max_chunks: self.max_chunks,
            allocator,
            _marker: PhantomData,
        }
    }

    /// Builds the pool.
    ///
    /// # Panics
    ///
    /// Panics if the requested `chunk_size` is `0` or greater than `2^31` (the
    /// largest `u32` whose next power of two is representable), if
    /// `chunk_size * max_chunks` exceeds the addressable ceiling — the `u32`
    /// slot-index limit, or (on 32-bit targets) the `usize` capacity of the
    /// pool's refcount — or if the per-chunk memory layout overflows.
    #[must_use]
    #[cold]
    pub fn build(self) -> Pool<T, A> {
        assert!(self.chunk_size >= 1, "chunk_size must be >= 1");
        assert!(self.chunk_size <= 1 << 31, "chunk_size must be <= 2^31");
        let chunk_size = self.chunk_size.next_power_of_two();
        if let Some(max) = self.max_chunks {
            let total = u64::from(chunk_size) * u64::from(max);
            // Slot indices must avoid `FREE_END`, and `1 + total` must fit in
            // the pool's `AtomicUsize` refcount.
            assert!(
                total <= MAX_POOL_SLOTS,
                "chunk_size * max_chunks exceeds the addressable slot/refcount ceiling"
            );
        }
        let layout = chunk_layout::<T>(chunk_size as usize).expect("chunk layout overflow");

        let inner = PoolInner {
            core: PoolCore {
                free_head: AtomicU32::new(FREE_END),
                pool_refcount: AtomicUsize::new(1),
                teardown: teardown_erased::<T, A>,
            },
            chunk_size,
            shift: chunk_size.trailing_zeros(),
            mask: chunk_size - 1,
            max_chunks: self.max_chunks,
            chunks_allocated: AtomicU32::new(0),
            #[cfg(feature = "stats")]
            bytes_allocated: AtomicUsize::new(0),
            chunk_layout: layout,
            directory: UnsafeCell::new(Vec::new()),
            allocator: self.allocator,
            _marker: PhantomData,
        };
        let raw = AllocBox::into_raw(AllocBox::new(inner));
        let inner = NonNull::new(raw).expect("Box::into_raw never returns a null pointer");
        Pool::from_inner(inner)
    }
}
