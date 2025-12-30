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
use new_zealand::nz;

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
    inner: Arc<GlobalPoolInner>,
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
            inner: Arc::new(GlobalPoolInner::new()),
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
    pub fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.inner.reserve(min_bytes)
    }
}

impl Memory for GlobalPool {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        self.reserve(min_bytes)
    }
}

#[derive(Debug)]
struct GlobalPoolInner {
    // This is guarded by a mutex because memory providers need to be thread-safe. The point is
    // not so much that memory will be requested from multiple threads (though it might) but that
    // even if memory is requested from a thread-specific pool, it may later be released on a
    // different thread, so the pool must be able to handle that.
    //
    // This is wrapped in an Arc because each pool item has a reference back to the pool that
    // contains it. This means there is a reference cycle in there! The pool can only be dropped
    // once all items in the pool have been returned to the pool. This is both good and bad.
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
    block_pool: Arc<Mutex<RawPinnedPool<MaybeUninit<NeutralBlock>>>>,
}

impl GlobalPoolInner {
    fn new() -> Self {
        Self {
            block_pool: Arc::new(Mutex::new(RawPinnedPool::new())),
        }
    }

    fn reserve(&self, min_bytes: usize) -> crate::BytesBuf {
        let block_count = min_bytes.div_ceil(BLOCK_SIZE_BYTES.get() as usize);

        let mut pool = self.block_pool.lock().expect(ERR_POISONED_LOCK);

        let blocks = iter::repeat_with(|| {
            let initialize_block = |place: &mut MaybeUninit<NeutralBlock>, handle: RawPooledMut<NeutralBlock>| {
                // The BlockMeta wants a shared handle, so we need to downgrade immediately.
                // Handles are not references so we can still create exclusive references
                // for as long as we can unsafely guarantee no aliasing violations exist.
                let handle = handle.into_shared();

                let meta = BlockMeta {
                    block_pool: Arc::clone(&self.block_pool),
                    handle,
                    ref_count: AtomicUsize::new(1),
                };

                in_place_initialize_block(place, meta);

                handle
            };

            // SAFETY: We are not allowed to dereference the handle until this returns (we do not)
            // and we are required to fully initialize the object before returning (we do).
            let handle = unsafe { insert_with_handle_to_self(&mut *pool, initialize_block) };

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
            let block_ref = unsafe { BlockRef::new(meta_ptr, &BLOCK_REF_FNS) };

            // SAFETY: We must guarantee exclusive ownership - yep, we do. As long as any BlockRef
            // clone is alive, the caller owns this block. We only return it to the pool when
            // all references have been released.
            unsafe { Block::new(capacity_ptr.cast(), BLOCK_SIZE_BYTES, block_ref) }
        })
        .take(block_count);

        BytesBuf::from_blocks(blocks)
    }
}

/// Fairly arbitrary choice just to get started with some value. We will eventually
/// want to converge with .NET `ArrayPool` most likely, as that is a "known good implementation".
const BLOCK_SIZE_BYTES: NonZero<BlockSize> = nz!(65_536);

/// Combination of memory capacity and metadata governing its lifecycle.
#[derive(Debug)]
struct NeutralBlock {
    /// Metadata about the block. Shared ownership by multiple instances of `BlockRef`.
    meta: BlockMeta,

    /// The actual memory capacity provided by the block.
    /// Ownership is controlled by custom logic of `SpanBuilder` and `Span`.
    ///
    /// We wrap it all in a great big `UnsafeCell` to indicate to Rust that this is
    /// mutated in a shared-referenced manner with custom logic controlling ownership.
    memory: UnsafeCell<[MaybeUninit<u8>; BLOCK_SIZE_BYTES.get() as usize]>,
}

// SAFETY: Usage of the the memory capacity is controlled on a byte slice level by custom logic
// in Span and SpanBuilder, which work together to ensure that only immutable slices are shared
// and mutable slices are exclusively owned, ensuring no concurrent access to them. The metadata
// is either naturally thread-safe or is protected by atomics as part of the block handles,
// depending on the exact field.
unsafe impl Sync for NeutralBlock {}

/// Carries the metadata of the block, used to manage lifecycle and return it to the pool.
#[derive(Debug)]
struct BlockMeta {
    /// The pool that this block is to be returned to. See comments in `GlobalPoolInner`.
    block_pool: Arc<Mutex<RawPinnedPool<MaybeUninit<NeutralBlock>>>>,

    /// The block has a handle to itself. This is used by the last reference to return
    /// the capacity to the pool once the last reference is dropped.
    handle: RawPooled<NeutralBlock>,

    /// Whoever decrements this to zero is responsible for returning the block to the pool.
    ref_count: AtomicUsize,
}

#[cfg_attr(test, mutants::skip)] // Failure to initialize can violate memory safety.
fn in_place_initialize_block(block: &mut MaybeUninit<NeutralBlock>, meta: BlockMeta) {
    let block_ptr = block.as_mut_ptr();

    // SAFETY: We are making a pointer to a known field at a compiler-guaranteed offset.
    let meta_ptr = unsafe { block_ptr.byte_add(offset_of!(NeutralBlock, meta)) }.cast::<BlockMeta>();

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

const BLOCK_REF_FNS: BlockRefVTable<BlockMeta> = BlockRefVTable::from_trait();

// SAFETY: We must guarantee thread-safety. We do - atomics are used for
// reference counting and the pool is protected by a mutex.
unsafe impl BlockRefDynamic for BlockMeta {
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

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use std::thread;

    use static_assertions::assert_impl_all;

    use super::*;
    use crate::mem::MemoryShared;

    assert_impl_all!(GlobalPool: MemoryShared);

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
        const SEQUENCE_SIZE_BYTES: BlockSize = 1000;

        // We grab a block of memory and split the single block into multiple views piece by piece.
        let memory = GlobalPool::new();

        let mut buf = memory.reserve(BLOCK_SIZE_BYTES.get() as usize);

        let mut views = Vec::new();

        while buf.remaining_capacity() > 0 {
            #[expect(clippy::cast_possible_truncation, reason = "intentionally truncating")]
            let value = views.len() as u8;

            buf.put_byte_repeated(value, (SEQUENCE_SIZE_BYTES as usize).min(buf.remaining_capacity()));

            // Sanity check against silly mutations.
            debug_assert!(!buf.is_empty());

            views.push(buf.consume_all());
        }

        let expected_count = BLOCK_SIZE_BYTES.get().div_ceil(SEQUENCE_SIZE_BYTES);
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

        let mut sb = memory.reserve(BLOCK_SIZE_BYTES.get() as usize);
        sb.put_byte_repeated(42, BLOCK_SIZE_BYTES.get() as usize);

        let sequence = sb.consume_all();

        thread::spawn({
            move || {
                drop(sequence);

                assert!(memory.inner.block_pool.lock().unwrap().is_empty());
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

        let mut sb = memory.reserve(SIZE_10MB);

        sb.put_slice(pattern.as_slice());

        let sequence = sb.consume_all();

        // Verify the sequence contains the expected pattern
        assert_eq!(sequence.len(), SIZE_10MB);
        assert_eq!(sequence, pattern.as_slice());
    }

    #[test]
    #[cfg(not(miri))] // Miri runtime scales with memory access size, so this takes forever.
    fn two_large_sequences_different_patterns() {
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

        let sequence1 = sb1.consume_all();
        let sequence2 = sb2.consume_all();

        // Verify both sequences have correct size
        assert_eq!(sequence1.len(), SIZE_10MB);
        assert_eq!(sequence2.len(), SIZE_10MB);

        assert_eq!(sequence1, pattern1.as_slice());
        assert_eq!(sequence2, pattern2.as_slice());
    }
}
