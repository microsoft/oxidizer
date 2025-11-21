// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::{Layout, alloc, dealloc};
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::ptr::NonNull;
use std::sync::atomic::{self, AtomicUsize};

use crate::{Block, BlockRef, BlockRefDynamic, BlockRefVTable, BlockSize};

/// Allocates a new memory block of the given length from the Rust global allocator.
#[must_use]
pub fn allocate(len: NonZero<BlockSize>) -> Block {
    // This will become the inner state behind the BlockRef instances.
    let block_ptr = new_block(len);

    // SAFETY: Yeah, there is a StdAllocBlock behind the pointer because we just created it.
    // We only ever created shared references to it - the data in this structure is immutable.
    let block = unsafe { block_ptr.as_ref() };

    // SAFETY: block_ptr must remain valid for reads and writes until drop()
    // is called via the dynamic fns. Yep, it does - the dynamic impl type takes ownership.
    let block_ref = unsafe { BlockRef::new(block_ptr, &BLOCK_REF_FNS) };

    // SAFETY: We promise that this block is exclusively owned, we do not share it to anyone.
    unsafe { Block::new(block.ptr, block.len, block_ref) }
}

/// A memory block that simply represents an allocation made via the Rust global allocator.
///
/// This can be used both to control test conditions in tests of this package and also as
/// the building block in memory providers that rely on the Rust global allocator.
#[derive(Debug)]
struct StdAllocBlock {
    ptr: NonNull<MaybeUninit<u8>>,
    len: NonZero<BlockSize>,
    ref_count: AtomicUsize,
}

// SAFETY: We must guarantee thread-safety. We do.
unsafe impl BlockRefDynamic for StdAllocBlock {
    type State = Self;

    #[cfg_attr(test, mutants::skip)] // Mutations can violate memory safety and cause UB.
    fn clone(state_ptr: NonNull<Self::State>) -> NonNull<Self::State> {
        // SAFETY: The state pointer is always valid for reads.
        // We only ever created shared references to it - the data in this structure is immutable.
        let state = unsafe { state_ptr.as_ref() };

        // Relaxed because incrementing reference count is independent of any other state.
        state.ref_count.fetch_add(1, atomic::Ordering::Relaxed);

        // We reuse the state for all clones.
        state_ptr
    }

    #[cfg_attr(test, mutants::skip)] // Mutations can violate memory safety and cause UB.
    fn drop(state_ptr: NonNull<Self::State>) {
        // SAFETY: The state pointer is always valid for reads.
        // We only ever created shared references to it - the data in this structure is immutable.
        let state = unsafe { state_ptr.as_ref() };

        // Release because we are releasing the synchronization block of the state.
        if state.ref_count.fetch_sub(1, atomic::Ordering::Release) != 1 {
            return;
        }

        // This was the last reference, so we can deallocate the block.

        // Ensure that we have observed all writes into the block from other threads.
        // On x86 this does nothing but on weaker memory models writes could be delayed.
        atomic::fence(atomic::Ordering::Acquire);

        // First we deallocate the block's capacity.
        // SAFETY: Layout must match between allocation and deallocation. It does.
        unsafe { dealloc(state.ptr.as_ptr().cast(), byte_array_layout(state.len)) };

        // Then we deallocate the block object itself.
        // SAFETY: Layout must match between allocation and deallocation. It does.
        unsafe {
            dealloc(state_ptr.as_ptr().cast(), BLOCK_LAYOUT);
        }
    }
}

const BLOCK_REF_FNS: BlockRefVTable<StdAllocBlock> = BlockRefVTable::from_trait();

fn byte_array_layout(len: NonZero<BlockSize>) -> Layout {
    Layout::array::<u8>(len.get() as usize).expect("the layout of a byte array can always be determined")
}

// SAFETY: We are asking for the layout of a valid Rust type and passing its natural size
// and alignment - nothing can go wrong.
const BLOCK_LAYOUT: Layout = unsafe { Layout::from_size_align_unchecked(size_of::<StdAllocBlock>(), align_of::<StdAllocBlock>()) };

fn new_block(len: NonZero<BlockSize>) -> NonNull<StdAllocBlock> {
    // TODO: Can we merge these two allocations to optimize this more?

    // SAFETY: Layout must be non-zero and otherwise sane.
    // It is - we use NonZero for len to ensure non-zero size.
    let capacity_ptr = NonNull::new(unsafe { alloc(byte_array_layout(len)) })
        .expect("we do not intend to handle failed allocations - they are fatal")
        .cast::<MaybeUninit<u8>>();

    // SAFETY: Layout must be non-zero and otherwise sane.
    // It is - we know that Block is a normal type and we have a normal layout for it.
    let block_ptr = NonNull::new(unsafe { alloc(BLOCK_LAYOUT) })
        .expect("we do not intend to handle failed allocations - they are fatal")
        .cast::<StdAllocBlock>();

    let block = StdAllocBlock {
        ptr: capacity_ptr,
        len,
        ref_count: AtomicUsize::new(1),
    };

    // SAFETY: We just allocated that memory with a proper layout, so it is both valid
    // for writes and properly aligned.
    unsafe { block_ptr.write(block) };

    block_ptr
}
