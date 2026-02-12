// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::{Layout, alloc, dealloc};
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::sync::atomic::{self, AtomicUsize};

use crate::mem::{Block, BlockMeta, BlockRef, BlockRefDynamic, BlockRefDynamicWithMeta, BlockRefVTable, BlockSize};

/// A memory block for testing purposes, exposing unusual functionality that does not make
/// sense for a real memory block (e.g. attaching arbitrary object as metadata).
///
/// The memory block is owned by the caller. Dropping the last `BlockRef` to this block does
/// nothing - the caller can choose when to drop the block. This facilitates testing of
/// `BlockRef` lifetime handling.
pub(crate) struct TestMemoryBlock {
    capacity_ptr: NonNull<MaybeUninit<u8>>,
    len: NonZero<BlockSize>,

    meta: Option<Box<dyn BlockMeta>>,

    ref_count: AtomicUsize,
}

impl TestMemoryBlock {
    /// # Safety
    ///
    /// The caller is not allowed to drop the block until all `BlockRef` to it are dropped.
    pub(crate) unsafe fn new(len: NonZero<BlockSize>, meta: Option<Box<dyn BlockMeta>>) -> Self {
        // SAFETY: Layout must be non-zero and otherwise sane.
        // It is - we use NonZero for len to ensure non-zero size.
        let capacity_ptr = NonNull::new(unsafe { alloc(byte_array_layout(len)) })
            .expect("we do not intend to handle failed allocations - they are fatal")
            .cast::<MaybeUninit<u8>>();

        Self {
            capacity_ptr,
            len,
            meta,
            ref_count: AtomicUsize::new(0),
        }
    }

    pub(crate) fn to_block_ref(self: Pin<&Self>) -> BlockRef {
        let function_table = if self.meta.is_some() {
            &BLOCK_WITHOUT_MEMORY_FNS_WITH_META
        } else {
            &BLOCK_WITHOUT_MEMORY_FNS
        };

        // Relaxed because reference count increment is independent of any state.
        self.ref_count.fetch_add(1, atomic::Ordering::Relaxed);

        let state_ptr = NonNull::from(self.get_ref());

        // SAFETY: state_ptr must remain valid for reads and writes until drop()
        // is called via the dynamic fns. This is guaranteed by the `new()` safety requirements.
        // We only ever use shared references to the state_ptr - no exclusive references are
        // ever created. In theory, the owner could create one but Miri would yell at them.
        unsafe { BlockRef::new(state_ptr, function_table) }
    }

    /// # Safety
    ///
    /// This may only be called once per block lifetime, as the returned instance takes
    /// exclusive ownership of the contents of the block's memory capacity.
    pub(crate) unsafe fn to_block(self: Pin<&Self>) -> Block {
        // SAFETY: Forwarding safety requirements of the caller.
        unsafe { Block::new(self.capacity_ptr, self.len, self.to_block_ref()) }
    }

    pub(crate) fn ref_count(&self) -> usize {
        self.ref_count.load(atomic::Ordering::Relaxed)
    }
}

impl Drop for TestMemoryBlock {
    fn drop(&mut self) {
        // SAFETY: Layout must match between allocation and deallocation. It does.
        unsafe {
            dealloc(self.capacity_ptr.as_ptr().cast(), byte_array_layout(self.len));
        }
    }
}

// SAFETY: We must ensure thread-safety of the implementation. We do.
unsafe impl BlockRefDynamic for TestMemoryBlock {
    type State = Self;

    fn clone(state_ptr: NonNull<Self::State>) -> NonNull<Self::State> {
        // SAFETY: The state pointer is always valid for reads.
        // We only ever use shared references to the state_ptr - no exclusive references are
        // ever created. In theory, the owner could create one but Miri would yell at them.
        let state = unsafe { state_ptr.as_ref() };

        state.ref_count.fetch_add(1, atomic::Ordering::Relaxed);

        // Reuse the state for all clones.
        state_ptr
    }

    fn drop(state_ptr: NonNull<Self::State>) {
        // SAFETY: The state pointer is always valid for reads.
        // We only ever use shared references to the state_ptr - no exclusive references are
        // ever created. In theory, the owner could create one but Miri would yell at them.
        let state = unsafe { state_ptr.as_ref() };

        state.ref_count.fetch_sub(1, atomic::Ordering::Release);

        // We do not actually deallocate anything - the owner must do that themselves.
    }
}

// SAFETY: We must ensure thread-safety of the implementation. We do.
unsafe impl BlockRefDynamicWithMeta for TestMemoryBlock {
    fn meta(state_ptr: NonNull<Self::State>) -> NonNull<dyn BlockMeta> {
        // SAFETY: The state pointer is always valid for reads.
        // We only ever use shared references to the state_ptr - no exclusive references are
        // ever created. In theory, the owner could create one but Miri would yell at them.
        let state = unsafe { state_ptr.as_ref() };

        let as_ref_box = state.meta.as_ref().expect("meta must be set if using with-meta function table");

        // Safe to pointerize because the parent API contract requires that
        // this has a lifetime that is a subset of the lifetime of `data`.
        let as_any: &dyn BlockMeta = as_ref_box.as_ref();

        NonNull::new(ptr::from_ref(as_any).cast_mut()).expect("field of non-null is non-null")
    }
}

fn byte_array_layout(len: NonZero<BlockSize>) -> Layout {
    Layout::array::<u8>(len.get() as usize).expect("the layout of a byte array can always be determined")
}

const BLOCK_WITHOUT_MEMORY_FNS: BlockRefVTable<TestMemoryBlock> = BlockRefVTable::from_trait();

const BLOCK_WITHOUT_MEMORY_FNS_WITH_META: BlockRefVTable<TestMemoryBlock> = BlockRefVTable::from_trait_with_meta();
