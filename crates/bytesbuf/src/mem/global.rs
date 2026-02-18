// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::UnsafeCell;
use std::mem::{self, MaybeUninit, offset_of};
use std::num::NonZero;
use std::ptr::NonNull;
use std::sync::atomic::{self, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::{iter, ptr};

use infinity_pool::{RawPinnedPool, RawPooled, RawPooledMut};
use nm::{Event, Magnitude};

use crate::BytesBuf;
use crate::constants::ERR_POISONED_LOCK;
use crate::mem::{Block, BlockRef, BlockRefDynamic, BlockRefVTable, BlockSize, Memory};

/// A memory pool that obtains memory from the Rust global allocator.
///
/// For clarity, the pool itself is not in any way global - rather the word "global" in the name
/// refers to the fact that all the memory capacity is obtained from the Rust global memory allocator.
#[doc = include_str!("../../doc/snippets/choosing_memory_provider.md")]
#[derive(Clone, Debug)]
pub struct GlobalPool {
    inner: thread_aware::Arc<GlobalPoolInner, thread_aware::PerCore>,
}

impl thread_aware::ThreadAware for GlobalPool {
    fn relocated(self, source: thread_aware::affinity::MemoryAffinity, destination: thread_aware::affinity::PinnedAffinity) -> Self {
        Self {
            inner: self.inner.relocated(source, destination),
        }
    }
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

type SubPool<const SIZE: usize> = Arc<Mutex<RawPinnedPool<MaybeUninit<NeutralBlock<SIZE>>>>>;

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
    // Each sub-pool is wrapped in an Arc because each pool item has a reference back to the sub-pool
    // that contains it. This means there is a reference cycle in there! The sub-pool can only be
    // dropped once all items in the sub-pool have been returned. This is both good and bad.
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
    // We delay-initialize each item in the pool because the items own their own pool handles.
    // Therefore, they must be wrapped in MaybeUninit. Once actually in use, always guaranteed
    // to be initialized, however - we only use MaybeUninit capabilities during insertion.
    pool_1k: SubPool<1024>,
    pool_4k: SubPool<4096>,
    pool_16k: SubPool<16_384>,
    pool_64k: SubPool<65_536>,
}

impl GlobalPoolInner {
    fn new() -> Self {
        Self {
            pool_1k: Arc::new(Mutex::new(RawPinnedPool::new())),
            pool_4k: Arc::new(Mutex::new(RawPinnedPool::new())),
            pool_16k: Arc::new(Mutex::new(RawPinnedPool::new())),
            pool_64k: Arc::new(Mutex::new(RawPinnedPool::new())),
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

    let mut pool = pool_arc.lock().expect(ERR_POISONED_LOCK);
    pool.reserve(block_count);

    let blocks = iter::repeat_with(|| allocate_block(&mut *pool, pool_arc, vtable)).take(block_count);

    BytesBuf::from_blocks(blocks)
}

/// Allocates a single block from the given sub-pool.
///
/// The caller is responsible for locking the pool and observing metrics.
fn allocate_block<const SIZE: usize>(
    pool: &mut RawPinnedPool<MaybeUninit<NeutralBlock<SIZE>>>,
    pool_arc: &SubPool<SIZE>,
    vtable: &'static BlockRefVTable<BlockMeta<SIZE>>,
) -> Block {
    let initialize_block = |place: &mut MaybeUninit<NeutralBlock<SIZE>>, handle: RawPooledMut<NeutralBlock<SIZE>>| {
        // The BlockMeta wants a shared handle, so we need to downgrade immediately.
        // Handles are not references so we can still create exclusive references
        // for as long as we can unsafely guarantee no aliasing violations exist.
        let handle = handle.into_shared();

        let meta = BlockMeta {
            block_pool: Arc::clone(pool_arc),
            handle,
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
    /// The pool that this block is to be returned to. See comments in `GlobalPoolInner`.
    block_pool: SubPool<SIZE>,

    /// The block has a handle to itself. This is used by the last reference to return
    /// the capacity to the pool once the last reference is dropped.
    handle: RawPooled<NeutralBlock<SIZE>>,

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

/// Inserts a `T` into a pool of `MaybeUninit<T>`, providing the object
/// its own handle on creation.
///
/// After this method returns, the object in the pool is guaranteed to be initialized.
/// Correspondingly, the handle it is provided is stripped of the `MaybeUninit` wrapper.
///
/// # Safety
///
/// The `initialize` function must fully initialize the object before returning.
///
/// The provided handle may only be dereferenced after this function returns.
unsafe fn insert_with_handle_to_self<F, T, R>(pool: &mut RawPinnedPool<MaybeUninit<T>>, initialize: F) -> R
where
    F: FnOnce(&mut MaybeUninit<T>, RawPooledMut<T>) -> R,
{
    // SAFETY: We are required to fully initialize the object. We "do" because the entire
    // object `T` is wrapped in MaybeUninit, so we are not required to do anything at all.
    // We do this purely to get the handle, because we need the handle to do the real
    // initialization.
    let handle = unsafe { pool.insert_with(|_| {}) };

    // SAFETY: This is the only reference that exists, ensuring no conflicts.
    let object_uninit = unsafe { handle.ptr().as_mut() };

    // SAFETY: After this function returns, the object is guaranteed to be initialized.
    // The provided handle may only be dereferenced after this function returns. Therefore,
    // the handle can only be used to access the object when already initialized.
    let handle = unsafe { mem::transmute::<RawPooledMut<MaybeUninit<T>>, RawPooledMut<T>>(handle) };

    initialize(object_uninit, handle)
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

        // We make local copies of what we need because the next part will invalidate `state`.
        // We are essentially inside a ManuallyDrop<state> here, just not expressed as such.
        let handle = state.handle;

        // SAFETY: We are moving out of state part of manual drop logic.
        let pool = unsafe { ptr::read(&raw const state.block_pool) };

        let mut pool = pool.lock().expect(ERR_POISONED_LOCK);

        // SAFETY: We must promise that it is no longer references and is still in the pool.
        // Sure, we can promise that because tracking that is the entire purpose of this type.
        unsafe {
            pool.remove(handle);
        }
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
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use std::thread;

    use static_assertions::assert_impl_all;

    use super::*;
    use crate::mem::MemoryShared;

    assert_impl_all!(GlobalPool: MemoryShared);
    assert_impl_all!(GlobalPool: thread_aware::ThreadAware);

    /// Helper to assert all sub-pools are empty.
    fn assert_all_pools_empty(inner: &GlobalPoolInner) {
        assert!(inner.pool_1k.lock().unwrap().is_empty());
        assert!(inner.pool_4k.lock().unwrap().is_empty());
        assert!(inner.pool_16k.lock().unwrap().is_empty());
        assert!(inner.pool_64k.lock().unwrap().is_empty());
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
    #[cfg(not(miri))] // Miri runtime scales with memory access size, so this takes forever.
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
    #[cfg(not(miri))] // Miri runtime scales with memory access size, so this takes forever.
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
        use thread_aware::ThreadAware;
        use thread_aware::affinity::pinned_affinities;

        let affinities = pinned_affinities(&[2]);
        let source = affinities[0].into();
        let destination = affinities[1];

        let memory = GlobalPool::new();

        // Allocate from the original pool.
        let mut buf = memory.reserve(100);
        buf.put_byte(42);
        let view = buf.consume_all();
        assert_eq!(view.first_slice()[0], 42);

        // Relocate the pool to a different affinity.
        let relocated_memory = memory.relocated(source, destination);

        // The relocated pool should work independently.
        let mut buf2 = relocated_memory.reserve(200);
        buf2.put_byte(99);
        let view2 = buf2.consume_all();
        assert_eq!(view2.first_slice()[0], 99);
    }
}
