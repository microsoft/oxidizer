// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::UnsafeCell;
use std::iter;
use std::mem::{MaybeUninit, offset_of};
use std::num::NonZero;
use std::ptr::NonNull;
use std::sync::atomic::{self, AtomicUsize};
use std::sync::{Arc, Mutex};

use nm::{Event, Magnitude};
use plurality::Pool;
use thread_aware::ThreadAware;

use crate::BytesBuf;
use crate::constants::ERR_POISONED_LOCK;
use crate::mem::{Block, BlockRef, BlockRefDynamic, BlockRefVTable, BlockSize, Memory};

/// A memory pool that obtains memory from the Rust global allocator.
///
/// For clarity, the pool itself is not in any way global - rather the word "global" in the name
/// refers to the fact that all the memory capacity is obtained from the Rust global memory allocator.
///
/// Clones of this type are equivalent and share the same pool of memory as long as they remain on the same thread.
#[doc = include_str!("../../docs/snippets/choosing_memory_provider.md")]
///
/// # Multithreaded use
///
/// Instances of this type should not be manually moved across threads (e.g. by capturing in a closure and
/// handing to `thread::spawn()` or `tokio::spawn()`). While the pool will still operate correctly, it will
/// suffer degraded performance in all clones from the same family.
///
/// This type is [thread-aware]. If moved across threads using thread-aware APIs, the performance
/// penalty is not incurred. If no suitable thread-aware API is available, use a thread-local pool
/// via the `thread_local!` macro.
///
/// [thread-aware]: https://docs.rs/thread_aware
#[derive(Clone, Debug, ThreadAware)]
pub struct GlobalPool {
    inner: thread_aware::Arc<GlobalPoolInner, thread_aware::PerCore>,
}

impl GlobalPool {
    /// Creates a new instance of the global memory pool.
    ///
    /// # Efficiency
    ///
    /// Each call to `new()` allocates a separate instance of the pool with its own memory capacity,
    /// so avoid creating multiple instances if you can reuse an existing one.
    ///
    /// Clones of a pool act as shared handles and share the memory capacity - feel free to clone
    /// as needed for convenient referencing purposes.
    #[must_use]
    #[expect(
        clippy::new_without_default,
        reason = "to avoid accidental confusion with some 'default' global memory pool, which does not exist"
    )]
    pub fn new() -> Self {
        Self {
            inner: thread_aware::Arc::<_, thread_aware::PerCore>::new(GlobalPoolInner::new),
        }
    }

    /// Reserves at least `min_bytes` bytes of memory capacity.
    ///
    /// Returns an empty [`BytesBuf`] that can be used to fill the reserved memory with data.
    ///
    /// The memory provider may provide more memory than requested.
    ///
    /// The memory reservation request will always be fulfilled, obtaining more memory from the
    /// operating system if necessary.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`BytesBuf`]
    /// with zero or more bytes of capacity.
    ///
    /// # Panics
    ///
    /// May panic if the operating system runs out of memory.
    #[must_use]
    #[inline]
    pub fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.inner.reserve(min_bytes)
    }
}

impl Memory for GlobalPool {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.reserve(min_bytes)
    }
}

type SubPool<const SIZE: usize> = Arc<Mutex<Pool<NeutralBlock<SIZE>>>>;

/// A block's handle to its own pool slot, used by the last reference to return
/// the block to the pool. It is a raw pointer produced by
/// [`plurality::Box::into_raw`]; reconstructing and dropping the box on the last
/// release returns the slot (via plurality's lock-free reclaim path).
#[derive(Clone, Copy, Debug)]
struct BlockHandle<const SIZE: usize>(NonNull<NeutralBlock<SIZE>>);

// SAFETY: the handle only ever reconstructs the owning box to free the slot,
// which plurality supports from any thread (the free list is atomic MPSC), and
// the block's lifecycle is governed by the atomic `ref_count`. It carries no
// thread-affinity, so it is safe to move and share across threads — mirroring
// the `Send + Sync` raw pool handle it replaces.
unsafe impl<const SIZE: usize> Send for BlockHandle<SIZE> {}
// SAFETY: see the `Send` justification above.
unsafe impl<const SIZE: usize> Sync for BlockHandle<SIZE> {}

/// Creates a sub-pool for `SIZE`-byte blocks with a chunk size that keeps each
/// chunk near a uniform footprint regardless of block size, so growth
/// granularity is consistent across the four sub-pools.
fn new_block_pool<const SIZE: usize>() -> Pool<NeutralBlock<SIZE>> {
    const TARGET_CHUNK_BYTES: usize = 64 * 1024;
    #[expect(clippy::cast_possible_truncation, reason = "slot count is bounded by TARGET / SIZE, always small")]
    let slots_per_chunk = (TARGET_CHUNK_BYTES / SIZE).max(1) as u32;
    Pool::builder().chunk_size(slots_per_chunk).build()
}

#[derive(Debug)]
#[expect(
    clippy::struct_field_names,
    reason = "pool_ prefix provides clarity for the size-differentiated sub-pools"
)]
struct GlobalPoolInner {
    // Each sub-pool is guarded by its own mutex because memory providers need to be thread-safe.
    // The point is not so much that memory will be requested from multiple threads (though it
    // might), but that even if memory is requested from a thread-specific pool, it may later be
    // released on a different thread, so the pool must be able to handle that.
    //
    // We intentionally use a plain mutex here to keep the code simple. We never expect the mutex
    // to be contended because we target a thread-isolated architecture (any single pool is
    // effectively owned by one thread/core at a time). We have measured that, at least on the x64
    // platform, an uncontended mutex is sufficiently cheap to be almost an annotation, so there is
    // no need to reach for a more complex lock-free or per-core fast-path design. The mutex only
    // guards *allocation*; blocks are returned to the pool lock-free (plurality's reclaim path is
    // an atomic MPSC free list), so a block may be released on any thread without taking the lock.
    //
    // Each sub-pool is wrapped in an Arc so a block can be released after the `GlobalPool` that
    // created it is gone. We do not need an explicit back-pointer cycle for that: each block is
    // handed out as a leaked `plurality::Box` (via `into_raw`), which retains a pool reference
    // count. That keeps the pool's backing storage alive until the block is returned, even if the
    // `GlobalPool` and its `Pool` handles have already been dropped. This is both good and bad.
    //
    // On the upside, it ensures that even if whoever created the pool (e.g. an application
    // framework or a test harness) drops the pool when the memory is still in use (e.g. by some
    // background I/O that was not properly terminated), we do not at least have dangling pointers
    // to released memory.
    //
    // On the downside, if there is a simple resource leak where some memory was released without
    // correctly returning it to the pool, we will never find out because it will look like that
    // memory is still in use. We might be able to supplement this with metrics to help detect
    // mysteriously growing pools, thereby mitigating this risk somewhat.
    //
    // We delay-initialize each block because it owns its own self-handle: the block is reserved
    // uninitialized (`alloc_uninit_box`), its self-pointer is captured, and only then is the block
    // written. Once in use it is always initialized.
    pool_1k: SubPool<1024>,
    pool_4k: SubPool<4096>,
    pool_16k: SubPool<16_384>,
    pool_64k: SubPool<65_536>,
}

impl GlobalPoolInner {
    fn new() -> Self {
        INSTANCES_CREATED.with(Event::observe_once);

        Self {
            pool_1k: Arc::new(Mutex::new(new_block_pool())),
            pool_4k: Arc::new(Mutex::new(new_block_pool())),
            pool_16k: Arc::new(Mutex::new(new_block_pool())),
            pool_64k: Arc::new(Mutex::new(new_block_pool())),
        }
    }

    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        RESERVATION_REQUESTED_SIZE.with(|e| e.observe(min_bytes));

        if min_bytes == 0 {
            return BytesBuf::new();
        }

        // Pick the smallest sub-pool that fits, then use only that block size for the entire
        // reservation. For requests exceeding the largest block size, use multiple blocks of
        // the largest size. Using uniform block sizes avoids imbalances when repeated
        // reservations are not perfectly aligned with block size boundaries.
        if min_bytes <= 1024 {
            allocate_uniform::<1024>(&self.pool_1k, &BLOCK_REF_FNS_1K, min_bytes)
        } else if min_bytes <= 4096 {
            allocate_uniform::<4096>(&self.pool_4k, &BLOCK_REF_FNS_4K, min_bytes)
        } else if min_bytes <= 16_384 {
            allocate_uniform::<16_384>(&self.pool_16k, &BLOCK_REF_FNS_16K, min_bytes)
        } else {
            allocate_uniform::<65_536>(&self.pool_64k, &BLOCK_REF_FNS_64K, min_bytes)
        }
    }
}

/// Allocates one or more blocks of the same size to satisfy `min_bytes`.
fn allocate_uniform<const SIZE: usize>(
    pool_arc: &SubPool<SIZE>,
    vtable: &'static BlockRefVTable<BlockMeta<SIZE>>,
    min_bytes: usize,
) -> crate::BytesBuf {
    let block_count = min_bytes.div_ceil(SIZE);

    BLOCK_RENTED_SIZE.with(|e| e.batch(block_count).observe(SIZE));

    // The overwhelmingly common reservation fits in a single block. Building the buffer directly
    // from that block skips the iterator/collect/sum machinery the multi-block path requires.
    if block_count == 1 {
        // Scope the pool lock to block allocation only. The block carries its own handle back to
        // the pool, so the buffer can be built without holding the lock, keeping the critical
        // section minimal. This also avoids re-entrant locking should the block be released while
        // we still held the lock.
        let block = {
            let pool = pool_arc.lock().expect(ERR_POISONED_LOCK);
            allocate_block(&pool, vtable)
        };

        return BytesBuf::from_block(block);
    }

    let pool = pool_arc.lock().expect(ERR_POISONED_LOCK);

    let blocks = iter::repeat_with(|| allocate_block(&pool, vtable)).take(block_count);

    BytesBuf::from_blocks(blocks)
}

/// Allocates a single block from the given sub-pool.
///
/// The caller is responsible for locking the pool and observing metrics.
fn allocate_block<const SIZE: usize>(pool: &Pool<NeutralBlock<SIZE>>, vtable: &'static BlockRefVTable<BlockMeta<SIZE>>) -> Block {
    let initialize_block = |place: &mut MaybeUninit<NeutralBlock<SIZE>>, handle: NonNull<NeutralBlock<SIZE>>| {
        let meta = BlockMeta {
            handle: BlockHandle(handle),
            ref_count: AtomicUsize::new(1),
        };

        in_place_initialize_block(place, meta);

        handle
    };

    // SAFETY: We are not allowed to dereference the handle until this returns (we do not)
    // and we are required to fully initialize the object before returning (we do).
    let handle = unsafe { insert_with_handle_to_self(pool, initialize_block) };

    // SAFETY: After initialization (above), we only access the block via shared references.
    let block = unsafe { handle.as_ref() };

    // This is only accessed via shared references in the future.
    let meta_ptr = NonNull::from(&block.meta);

    // This is accessed via pointers and custom logic, which we "unlock" via UnsafeCell.
    //
    // SAFETY: UnsafeCell pointer is never null.
    let capacity_ptr = unsafe { NonNull::new_unchecked(block.memory.get()) };

    // SAFETY: meta_ptr must remain valid for reads and writes until drop()
    // is called via the dynamic fns. Yep, it does - the dynamic impl type takes ownership.
    // We only ever access it via shared references - no exclusive references are created.
    let block_ref = unsafe { BlockRef::new(meta_ptr, vtable) };

    #[expect(
        clippy::cast_possible_truncation,
        reason = "block sizes are always <= u32::MAX, as a core invariant of this crate"
    )]
    let block_size: NonZero<BlockSize> = const { NonZero::new(SIZE as u32).expect("block size is always a known non-zero constant") };

    // SAFETY: We must guarantee exclusive ownership - yep, we do. As long as any BlockRef
    // clone is alive, the caller owns this block. We only return it to the pool when
    // all references have been released.
    unsafe { Block::new(capacity_ptr.cast(), block_size, block_ref) }
}

/// Combination of memory capacity and metadata governing its lifecycle.
#[derive(Debug)]
struct NeutralBlock<const SIZE: usize> {
    /// Private metadata about the block. Shared ownership by multiple instances of `BlockRef`.
    meta: BlockMeta<SIZE>,

    /// The actual memory capacity provided by the block.
    /// Ownership is controlled by custom logic of `SpanBuilder` and `Span`.
    ///
    /// We wrap it all in a great big `UnsafeCell` to indicate to Rust that this is
    /// mutated in a shared-referenced manner with custom logic controlling ownership.
    memory: UnsafeCell<[MaybeUninit<u8>; SIZE]>,
}

// SAFETY: Usage of the the memory capacity is controlled on a byte slice level by custom logic
// in Span and SpanBuilder, which work together to ensure that only immutable slices are shared
// and mutable slices are exclusively owned, ensuring no concurrent access to them. The metadata
// is either naturally thread-safe or is protected by atomics as part of the block handles,
// depending on the exact field.
unsafe impl<const SIZE: usize> Sync for NeutralBlock<SIZE> {}

/// Carries the INTERNAL metadata of the block.
///
/// Used to manage lifecycle and return it to the pool.
///
/// This is not the public metadata that the `bytesbuf` API can expose - the global pool has no
/// public metadata.
#[derive(Debug)]
struct BlockMeta<const SIZE: usize> {
    /// The block has a handle to itself. This is used by the last reference to return
    /// the capacity to the pool once the last reference is dropped. The handle retains a
    /// plurality pool reference count, keeping the pool's backing storage alive until the
    /// block is returned (so a block may outlive the `GlobalPool`).
    handle: BlockHandle<SIZE>,

    /// Whoever decrements this to zero is responsible for returning the block to the pool.
    ref_count: AtomicUsize,
}

#[cfg_attr(test, mutants::skip)] // Failure to initialize can violate memory safety.
fn in_place_initialize_block<const SIZE: usize>(block: &mut MaybeUninit<NeutralBlock<SIZE>>, meta: BlockMeta<SIZE>) {
    let block_ptr = block.as_mut_ptr();

    // SAFETY: We are making a pointer to a known field at a compiler-guaranteed offset.
    let meta_ptr = unsafe { block_ptr.byte_add(offset_of!(NeutralBlock<SIZE>, meta)) }.cast::<BlockMeta<SIZE>>();

    // SAFETY: This is the matching field of the type we are initializing, so valid for writes.
    unsafe {
        meta_ptr.write(meta);
    }

    // We do not need to initialize the `memory` field - it starts as fully uninitialized.
}

/// RAII guard that owns the uninitialized pool slot returned by
/// [`plurality::Box::into_raw`]. If it is dropped before [`Self::into_raw`]
/// hands ownership back, it reconstructs and frees the box, returning the slot
/// to the pool on the unwinding path.
struct UninitBoxGuard<T> {
    ptr: NonNull<MaybeUninit<T>>,
}

impl<T> UninitBoxGuard<T> {
    fn into_raw(self) -> NonNull<MaybeUninit<T>> {
        let ptr = self.ptr;
        core::mem::forget(self);
        ptr
    }
}

impl<T> Drop for UninitBoxGuard<T> {
    fn drop(&mut self) {
        // SAFETY: the guard uniquely owns the pointer returned by into_raw and
        // reconstructs it only on unwind before initialization completed.
        unsafe { drop(plurality::Box::<MaybeUninit<T>>::from_raw(self.ptr)) };
    }
}

/// Reserves an uninitialized slot in `pool`, provides the object its own handle,
/// and runs `initialize` to fill it in.
///
/// After this function returns, the object in the slot is guaranteed to be
/// initialized, and the returned handle points at the initialized value. The
/// slot stays occupied — ownership is held by the raw handle until the block's
/// last reference reconstructs the box (via [`plurality::Box::from_raw`]) and
/// drops it.
///
/// # Safety
///
/// The `initialize` function must fully initialize the object before returning.
///
/// The provided handle may only be dereferenced after this function returns.
unsafe fn insert_with_handle_to_self<F, T, R>(pool: &Pool<T>, initialize: F) -> R
where
    F: FnOnce(&mut MaybeUninit<T>, NonNull<T>) -> R,
{
    // Preserve the full-provenance pointer returned by into_raw while a guard
    // retains ownership until initialization succeeds.
    let mut uninit_ptr = plurality::Box::into_raw(pool.alloc_uninit_box());
    let guard = UninitBoxGuard { ptr: uninit_ptr };

    // The self-handle is the value pointer (the slot address is stable). It may
    // only be dereferenced after `initialize` fully initializes the value.
    let handle = uninit_ptr.cast::<T>();

    // SAFETY: the guard uniquely owns the slot and no other reference exists.
    let result = initialize(unsafe { uninit_ptr.as_mut() }, handle);
    let owned = guard.into_raw().cast::<T>();
    debug_assert_eq!(owned, handle);
    result
}

const BLOCK_REF_FNS_1K: BlockRefVTable<BlockMeta<1024>> = BlockRefVTable::from_trait();
const BLOCK_REF_FNS_4K: BlockRefVTable<BlockMeta<4096>> = BlockRefVTable::from_trait();
const BLOCK_REF_FNS_16K: BlockRefVTable<BlockMeta<16_384>> = BlockRefVTable::from_trait();
const BLOCK_REF_FNS_64K: BlockRefVTable<BlockMeta<65_536>> = BlockRefVTable::from_trait();

// SAFETY: We must guarantee thread-safety. We do - atomics are used for
// reference counting and the pool is protected by a mutex.
unsafe impl<const SIZE: usize> BlockRefDynamic for BlockMeta<SIZE> {
    type State = Self;

    #[cfg_attr(test, mutants::skip)] // Mutations can violate memory safety and cause UB.
    fn clone(state_ptr: NonNull<Self::State>) -> NonNull<Self::State> {
        // SAFETY: The state pointer is always valid for reads.
        // We only ever access it via shared references - no exclusive references are created.
        let state = unsafe { state_ptr.as_ref() };

        // Relaxed because reference count increment is independent of any state.
        state.ref_count.fetch_add(1, atomic::Ordering::Relaxed);

        // We reuse the state for all clones.
        state_ptr
    }

    #[cfg_attr(test, mutants::skip)] // Mutations can violate memory safety and cause UB.
    fn drop(state_ptr: NonNull<Self::State>) {
        // SAFETY: The state pointer is always valid for reads.
        // We only ever access it via shared references - no exclusive references are created.
        let state = unsafe { state_ptr.as_ref() };

        // Release because we are releasing the synchronization block for the block state.
        if state.ref_count.fetch_sub(1, atomic::Ordering::Release) != 1 {
            return;
        }

        // This was the last reference, so we can release the block back to the pool.

        // Ensure that we have observed all writes into the block from other threads.
        // On x86 this does nothing but on weaker memory models writes could be delayed.
        atomic::fence(atomic::Ordering::Acquire);

        // Copy the self-handle out before we relinquish `state`: reconstructing and
        // dropping the box below runs the block's destructor, which invalidates it.
        let handle = state.handle;

        // Reconstruct the owning box from the block's self-handle and drop it.
        // Dropping the box runs the block's destructor and returns the slot to the
        // pool.
        //
        // The self-handle was produced by `into_raw` on a `Box<MaybeUninit<_>>`, so
        // we reconstruct with that same type before `assume_init` to match
        // `from_raw`'s "exact pointer returned by into_raw" contract.
        //
        // SAFETY: this is the block's own handle, produced by `into_raw` at
        // allocation; the last-reference check above guarantees it is reconstructed
        // exactly once and that no references to the block remain.
        let uninit = unsafe { plurality::Box::<MaybeUninit<NeutralBlock<SIZE>>>::from_raw(handle.0.cast()) };
        // SAFETY: the block was fully initialized before its first reference was
        // handed out, so the slot holds an initialized value.
        let block = unsafe { uninit.assume_init() };
        drop(block);
    }
}

// Histogram buckets for the size of each rented memory block, matching sub-pool sizes.
const BLOCK_SIZE_BUCKETS: &[Magnitude] = &[1024, 4096, 16_384, 65_536];

// Histogram buckets for the requested reservation size (more granular).
const RESERVATION_SIZE_BUCKETS: &[Magnitude] = &[
    0, 256, 512, 1024, 2048, 4096, 8192, 16_384, 32_768, 65_536, 131_072, 262_144, 524_288, 1_048_576,
];

thread_local! {
    static BLOCK_RENTED_SIZE: Event = Event::builder()
        .name("bytesbuf_global_pool_block_rented_size")
        .histogram(BLOCK_SIZE_BUCKETS)
        .build();

    static RESERVATION_REQUESTED_SIZE: Event = Event::builder()
        .name("bytesbuf_global_pool_reservation_requested_size")
        .histogram(RESERVATION_SIZE_BUCKETS)
        .build();

    // Counts how many GlobalPoolInner instances have been created. Each instance owns its own
    // memory capacity, so creating many of them defeats the purpose of pooling. In typical usage
    // with a thread-aware GlobalPool backed by thread_aware::PerCore, there is at most one
    // instance per core/affinity (for pinned worker threads), so application owners can use this
    // metric to detect if something is inadvertently creating an excessive number of pools instead
    // of reusing existing ones.
    static INSTANCES_CREATED: Event = Event::builder()
        .name("bytesbuf_global_pool_instances_total")
        .build();
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use std::thread;

    use static_assertions::assert_impl_all;
    use thread_aware::affinity::pinned_affinities;

    use super::*;
    use crate::mem::MemoryShared;

    assert_impl_all!(GlobalPool: MemoryShared);
    assert_impl_all!(GlobalPool: ThreadAware);

    /// Helper to assert all sub-pools are empty.
    fn assert_all_pools_empty(inner: &GlobalPoolInner) {
        assert!(inner.pool_1k.lock().unwrap().is_empty());
        assert!(inner.pool_4k.lock().unwrap().is_empty());
        assert!(inner.pool_16k.lock().unwrap().is_empty());
        assert!(inner.pool_64k.lock().unwrap().is_empty());
    }

    #[test]
    fn block_pool_chunk_sizes_target_64_kib() {
        fn assert_chunk_size<const SIZE: usize>(expected_slots: u64) {
            let pool = new_block_pool::<SIZE>();
            let slot = pool.alloc_uninit_box();
            assert_eq!(pool.capacity(), expected_slots);
            drop(slot);
        }

        assert_chunk_size::<1024>(64);
        assert_chunk_size::<4096>(16);
        assert_chunk_size::<16_384>(4);
        assert_chunk_size::<65_536>(1);
    }

    #[test]
    fn panicking_self_handle_initializer_returns_slot() {
        let pool = Pool::<u64>::builder().chunk_size(1).max_chunks(1).build();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: the callback never returns, so it cannot expose an
            // uninitialized value or dereference the provisional handle.
            unsafe {
                insert_with_handle_to_self::<_, u64, ()>(&pool, |_, _| {
                    panic!("initialization failed");
                });
            }
        }));

        assert!(result.is_err());
        assert!(pool.is_empty());
        drop(pool.alloc_box(7));
        assert!(pool.is_empty());
    }

    #[test]
    fn smoke_test() {
        let memory = GlobalPool::new();

        // Nothing to assert with 0-sized, any capacity is acceptable. Just as long as it works.
        _ = memory.reserve(0);

        let builder = memory.reserve(1);
        assert!(builder.capacity() >= 1);

        let builder = memory.reserve(100);
        assert!(builder.capacity() >= 100);

        let builder = memory.reserve(1000);
        assert!(builder.capacity() >= 1000);

        let builder = memory.reserve(10000);
        assert!(builder.capacity() >= 10000);

        let builder = memory.reserve(100_000);
        assert!(builder.capacity() >= 100_000);

        let builder = memory.reserve(1_000_000);
        assert!(builder.capacity() >= 1_000_000);
    }

    #[test]
    fn piece_by_piece() {
        const BLOCK_SIZE: usize = 65_536;
        const LEN_BYTES: BlockSize = 1000;

        // We grab a block of memory and split the single block into multiple views piece by piece.
        let memory = GlobalPool::new();

        let mut buf = memory.reserve(BLOCK_SIZE);

        let mut views = Vec::new();

        while buf.remaining_capacity() > 0 {
            #[expect(clippy::cast_possible_truncation, reason = "intentionally truncating")]
            let value = views.len() as u8;

            buf.put_byte_repeated(value, (LEN_BYTES as usize).min(buf.remaining_capacity()));

            // Sanity check against silly mutations.
            debug_assert!(!buf.is_empty());

            views.push(buf.consume_all());
        }

        #[expect(clippy::cast_possible_truncation, reason = "block size is small")]
        let expected_count = (BLOCK_SIZE as BlockSize).div_ceil(LEN_BYTES);
        assert_eq!(views.len(), expected_count as usize);

        assert!(!views.is_empty());

        for (i, sequence) in views.iter().enumerate() {
            #[expect(clippy::cast_possible_truncation, reason = "intentionally truncating")]
            let expected_value = i as u8;

            assert_eq!(sequence.first_slice()[0], expected_value);
        }
    }

    #[test]
    fn release_on_other_thread() {
        let memory = GlobalPool::new();

        let mut sub = memory.reserve(65_536);
        sub.put_byte_repeated(42, 65_536);

        let data = sub.consume_all();

        thread::spawn({
            move || {
                drop(data);
                assert_all_pools_empty(&memory.inner);
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri runtime scales with memory access size, so this takes forever.
    fn large_content_survives_trip() {
        const SIZE_10MB: usize = 10 * 1024 * 1024;

        let pattern = testing_aids::repeating_incrementing_bytes().take(SIZE_10MB).collect::<Vec<_>>();

        let memory = GlobalPool::new();

        let mut buf = memory.reserve(SIZE_10MB);

        buf.put_slice(pattern.as_slice());

        let message = buf.consume_all();

        // Verify the sequence contains the expected pattern
        assert_eq!(message.len(), SIZE_10MB);
        assert_eq!(message, pattern.as_slice());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Miri runtime scales with memory access size, so this takes forever.
    fn two_large_views_different_patterns() {
        const SIZE_10MB: usize = 10 * 1024 * 1024;

        let pattern1 = testing_aids::repeating_incrementing_bytes().take(SIZE_10MB).collect::<Vec<_>>();

        let pattern2 = testing_aids::repeating_reverse_incrementing_bytes()
            .take(SIZE_10MB)
            .collect::<Vec<_>>();

        let memory = GlobalPool::new();

        // Create first sequence with ascending pattern
        let mut sb1 = memory.reserve(SIZE_10MB);
        sb1.put_slice(pattern1.as_slice());

        // Create second sequence with descending pattern
        let mut sb2 = memory.reserve(SIZE_10MB);
        sb2.put_slice(pattern2.as_slice());

        let view1 = sb1.consume_all();
        let view2 = sb2.consume_all();

        // Verify both views have correct size
        assert_eq!(view1.len(), SIZE_10MB);
        assert_eq!(view2.len(), SIZE_10MB);

        assert_eq!(view1, pattern1.as_slice());
        assert_eq!(view2, pattern2.as_slice());
    }

    // --- Block selection tests ---

    #[test]
    fn small_reservation_uses_1k_block() {
        let memory = GlobalPool::new();

        let buf = memory.reserve(500);
        assert_eq!(buf.capacity(), 1024);
    }

    #[test]
    fn exact_1k_uses_1k_block() {
        let memory = GlobalPool::new();

        let buf = memory.reserve(1024);
        assert_eq!(buf.capacity(), 1024);
    }

    #[test]
    fn just_over_1k_uses_4k_block() {
        let memory = GlobalPool::new();

        let buf = memory.reserve(1025);
        assert_eq!(buf.capacity(), 4096);
    }

    #[test]
    fn exact_4k_uses_4k_block() {
        let memory = GlobalPool::new();

        let buf = memory.reserve(4096);
        assert_eq!(buf.capacity(), 4096);
    }

    #[test]
    fn just_over_4k_uses_16k_block() {
        let memory = GlobalPool::new();

        let buf = memory.reserve(4097);
        assert_eq!(buf.capacity(), 16_384);
    }

    #[test]
    fn exact_16k_uses_16k_block() {
        let memory = GlobalPool::new();

        let buf = memory.reserve(16_384);
        assert_eq!(buf.capacity(), 16_384);
    }

    #[test]
    fn just_over_16k_uses_64k_block() {
        let memory = GlobalPool::new();

        let buf = memory.reserve(16_385);
        assert_eq!(buf.capacity(), 65_536);
    }

    #[test]
    fn exact_64k_uses_64k_block() {
        let memory = GlobalPool::new();

        let buf = memory.reserve(65_536);
        assert_eq!(buf.capacity(), 65_536);
    }

    #[test]
    fn multi_block_uniform_64k() {
        // 70KB exceeds 64KB, so uses multiple 64KB blocks (all same size).
        let memory = GlobalPool::new();

        let buf = memory.reserve(70_000);
        assert_eq!(buf.capacity(), 65_536 * 2);
    }

    #[test]
    fn multi_block_many_64k() {
        // 200KB = ceil(200_000 / 65_536) = 4 blocks of 64KB.
        let memory = GlobalPool::new();

        let buf = memory.reserve(200_000);
        assert_eq!(buf.capacity(), 65_536 * 4);
    }

    #[test]
    fn zero_reservation() {
        let memory = GlobalPool::new();

        let buf = memory.reserve(0);
        // Zero reservation should not panic and should result in zero capacity (no blocks).
        assert_eq!(buf.capacity(), 0);
    }

    #[test]
    fn pool_isolation_small_block() {
        let memory = GlobalPool::new();

        let buf = memory.reserve(500);
        drop(buf);

        // Only the 1K pool was used and is now empty.
        assert!(memory.inner.pool_1k.lock().unwrap().is_empty());
        assert!(memory.inner.pool_4k.lock().unwrap().is_empty());
        assert!(memory.inner.pool_16k.lock().unwrap().is_empty());
        assert!(memory.inner.pool_64k.lock().unwrap().is_empty());
    }

    #[test]
    fn pool_isolation_multi_block() {
        // 70KB uses only 64KB blocks - only pool_64k is touched.
        let memory = GlobalPool::new();

        let buf = memory.reserve(70_000);
        drop(buf);

        assert_all_pools_empty(&memory.inner);
    }

    #[test]
    fn multi_block_just_over_64k() {
        // 66_036 > 64KB, so 2x 64KB blocks (uniform sizing).
        let memory = GlobalPool::new();

        let buf = memory.reserve(66_036);
        assert_eq!(buf.capacity(), 65_536 * 2);
    }

    #[test]
    fn relocated_pool_works() {
        let affinities = pinned_affinities(&[2]);
        let source = Some(affinities[0]);
        let destination = affinities[1];

        let mut memory = GlobalPool::new();

        // Allocate from the original pool.
        let mut buf = memory.reserve(100);
        buf.put_byte(42);
        let view = buf.consume_all();
        assert_eq!(view.first_slice()[0], 42);

        // Relocate the pool to a different affinity.
        memory.relocate(source, destination);

        // The relocated pool should work independently.
        let mut buf2 = memory.reserve(200);
        buf2.put_byte(99);
        let view2 = buf2.consume_all();
        assert_eq!(view2.first_slice()[0], 99);
    }
}
