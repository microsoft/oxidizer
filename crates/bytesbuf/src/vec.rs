// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::mem::MaybeUninit;
use std::num::NonZero;
use std::ptr::NonNull;
use std::sync::atomic::{self, AtomicUsize};

use smallvec::SmallVec;

use crate::mem::{Block, BlockRef, BlockRefDynamic, BlockRefVTable, BlockSize};
use crate::{BytesView, MAX_INLINE_SPANS, Span};

impl From<Vec<u8>> for BytesView {
    /// Converts a [`Vec<u8>`] instance into a `BytesView`.
    ///
    /// This operation is always zero-copy, though does cost a small dynamic allocation.
    fn from(value: Vec<u8>) -> Self {
        if value.is_empty() {
            return Self::new();
        }

        // A Vec<u8> instance may contain any number of bytes, same as a BytesView. However, each
        // block of memory inside BytesView is limited to BlockSize::MAX, which is a smaller size.
        // Therefore, we may need to chop up the Vec into smaller slices, so each slice fits in
        // a BlockSize. This iterator does the job.
        let vec_blocks = VecBlockIterator::new(value);

        let blocks = vec_blocks.map(|vec| {
            // SAFETY: We must treat the provided memory capacity as immutable. We do, only using
            // it to create a `BytesView` over the immutable data that already exists within.
            // Note that this requirement also extends down the stack - no code that runs in this
            // function is allowed to create an exclusive reference over the data of the `Vec`,
            // even if that exclusive reference is not used for writes (Miri will tell you if you
            // did it wrong).
            unsafe { non_empty_vec_to_immutable_block(vec) }
        });

        let spans = blocks.map(|block| {
            let mut span_builder = block.into_span_builder();

            #[expect(clippy::cast_possible_truncation, reason = "a span can never be larger than BlockSize")]
            let len = NonZero::new(span_builder.remaining_capacity() as BlockSize).expect("splitting Vec cannot yield zero-sized chunks");

            // SAFETY: We know that the data is already initialized; we simply declare this to the
            // SpanBuilder and get it to emit a completed Span from all its contents.
            unsafe {
                span_builder.advance(len.get() as usize);
            }

            span_builder.consume(len)
        });

        // NB! We cannot use `BytesBuf::from_blocks` because it is not guaranteed to use the
        // blocks in the same order as they are provided. Instead, we directly construct the inner
        // span array in the BytesView, which lets us avoid any temporary allocations and resizing.
        let mut spans_reversed: SmallVec<[Span; MAX_INLINE_SPANS]> = spans.collect();

        // Not ideal but 99.999% of the case this is a 1-element array, so it does not matter.
        spans_reversed.reverse();

        Self::from_spans_reversed(spans_reversed)
    }
}

/// An implementation of `BlockRef` that reuses immutable memory of an owned `Vec<u8>` instance.
struct VecBlock {
    // This field exists to keep the Vec alive. The data within is accessed directly via pointers.
    _inner: Vec<u8>,

    ref_count: AtomicUsize,
}

impl VecBlock {
    pub const fn new(inner: Vec<u8>) -> Self {
        Self {
            _inner: inner,
            ref_count: AtomicUsize::new(1),
        }
    }
}

// SAFETY: We must guarantee thread-safety. We do.
unsafe impl BlockRefDynamic for VecBlock {
    type State = Self;

    fn clone(state_ptr: NonNull<Self::State>) -> NonNull<Self::State> {
        // SAFETY: The state pointer is always valid for reads.
        // We only ever created shared references to the block state - it exists just to track the
        // reference count.
        let state = unsafe { state_ptr.as_ref() };

        // Relaxed because incrementing reference count is independent of any other state.
        state.ref_count.fetch_add(1, atomic::Ordering::Relaxed);

        // We reuse the same state between all clones.
        state_ptr
    }

    #[cfg_attr(test, mutants::skip)] // Impractical to test. Miri will inform about memory leaks.
    fn drop(state_ptr: NonNull<Self::State>) {
        // SAFETY: The state pointer is always valid for reads.
        // We only ever created shared references to the block state - it exists just to track the
        // reference count.
        let state = unsafe { state_ptr.as_ref() };

        // Release because we are releasing the synchronization block for the memory block state.
        if state.ref_count.fetch_sub(1, atomic::Ordering::Release) != 1 {
            return;
        }

        // This was the last reference, so we can deallocate the block.
        // All we need to do is deallocate the block object - dropping the Vec field
        // will cleanup the memory capacity provided by the Vec instance.

        // Ensure that we have observed all writes into the block from other threads.
        // On x86 this does nothing but on weaker memory models writes could be delayed.
        atomic::fence(atomic::Ordering::Acquire);

        // SAFETY: No more references exist, we can resurrect the object inside a Box and drop.
        drop(unsafe { Box::from_raw(state_ptr.as_ptr()) });
    }
}

/// # Panics
///
/// Panics if the `Vec` is larger than `BlockSize::MAX`.
///
/// # Safety
///
/// The block contents must be treated as immutable because once converted to a `BytesView`,
/// the contents of the `Vec` are accessed via shared references only.
unsafe fn non_empty_vec_to_immutable_block(vec: Vec<u8>) -> Block {
    assert!(!vec.is_empty());

    let len: BlockSize = vec
        .len()
        .try_into()
        .expect("length of Vec<u8> instance was greater than BlockSize::MAX");

    let capacity_ptr = NonNull::new(vec.as_ptr().cast_mut())
        .expect("guarded by 'is zero sized Vec' check upstream - non-empty Vec must have non-null capacity pointer")
        .cast::<MaybeUninit<u8>>();

    let len = NonZero::new(len).expect("guarded by 'is zero sized Vec' check upstream");

    let block_ptr = NonNull::new(Box::into_raw(Box::new(VecBlock::new(vec)))).expect("we just allocated it - it cannot possibly be null");

    // SAFETY: block_ptr must remain valid until the dynamic fns drop() is called. Yep, it does.
    // We only ever created shared references to the block state - it exists just to track the
    // reference count.
    let block_ref = unsafe { BlockRef::new(block_ptr, &BLOCK_REF_FNS) };

    // SAFETY: Block requires us to guarantee exclusive access. We actually cannot do that - this
    // memory block is shared and immutable, unlike many others! However, the good news is that this
    // requirement on Block exists to support mutation. As long as we never treat the block as
    // having mutable contents, we are fine with shared immutable access.
    unsafe { Block::new(capacity_ptr, len, block_ref) }
}

const BLOCK_REF_FNS: BlockRefVTable<VecBlock> = BlockRefVTable::from_trait();

/// Returns pieces of a `Vec<u8>` no greater than `BlockSize::MAX` in length.
struct VecBlockIterator {
    remaining: Vec<u8>,
}

impl VecBlockIterator {
    const fn new(vec: Vec<u8>) -> Self {
        Self { remaining: vec }
    }
}

impl Iterator for VecBlockIterator {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }

        let bytes_to_take = self.remaining.len().min(BlockSize::MAX as usize);

        // split_off splits at the given index, returning everything after that index.
        // We want to take the first `bytes_to_take` bytes, so we split_off at that index
        // and swap - what we split off becomes `remaining`, and what's left is what we return.
        let keep = self.remaining.split_off(bytes_to_take);
        let take = std::mem::replace(&mut self.remaining, keep);

        Some(take)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let blocks_remaining = self.remaining.len().div_ceil(BlockSize::MAX as usize);
        (blocks_remaining, Some(blocks_remaining))
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::testing::TransparentMemory;

    #[test]
    fn vec_into_sequence() {
        let vec = vec![1, 2, 3, 4, 5];
        let mut sequence: BytesView = vec.into();
        assert_eq!(sequence.len(), 5);

        assert_eq!(sequence.get_byte(), 1);
        assert_eq!(sequence.get_byte(), 2);
        assert_eq!(sequence.get_byte(), 3);
        assert_eq!(sequence.get_byte(), 4);
        assert_eq!(sequence.get_byte(), 5);

        assert!(sequence.is_empty());
    }

    #[test]
    fn zero_sized_vec() {
        let vec = Vec::<u8>::new();
        let sequence: BytesView = vec.into();

        assert_eq!(sequence.len(), 0);
        assert!(sequence.is_empty());
    }

    #[test]
    fn test_vec_to_sequence() {
        let vec = vec![b'H', b'e', b'l', b'l', b'o', b',', b' ', b'w', b'o', b'r', b'l', b'd', b'!'];

        let vec_data_ptr = vec.as_ptr();

        let sequence: BytesView = vec.into();

        assert_eq!(sequence.len(), 13);
        assert_eq!(sequence, b"Hello, world!");

        // We expect this to be zero-copy - Vec to BytesView always is.
        assert_eq!(sequence.first_slice().as_ptr(), vec_data_ptr);
    }

    #[test]
    fn test_sequence_to_bytes() {
        let memory = TransparentMemory::new();

        let sequence = BytesView::copied_from_slice(b"Hello, world!", &memory);

        let sequence_chunk_ptr = sequence.first_slice().as_ptr();

        let bytes = sequence.to_bytes();

        assert_eq!(bytes.as_ref(), b"Hello, world!");

        // We expect this to be zero-copy since we used the passthrough allocator.
        assert_eq!(bytes.as_ptr(), sequence_chunk_ptr);
    }

    #[test]
    fn test_multi_block_sequence_to_bytes() {
        let memory = TransparentMemory::new();

        let hello = BytesView::copied_from_slice(b"Hello, ", &memory);
        let world = BytesView::copied_from_slice(b"world!", &memory);
        let sequence = BytesView::from_views([hello, world]);

        let bytes = sequence.to_bytes();
        assert_eq!(bytes.as_ref(), b"Hello, world!");
    }

    #[test]
    fn test_giant_vec_to_sequence() {
        // This test requires at least 5 GB of memory to run. The publishing pipeline runs on a system
        // where this may not be available, so we skip this test in that environment.
        #[cfg(all(not(miri), any(target_os = "linux", target_os = "windows")))]
        if crate::testing::system_memory() < 10_000_000_000 {
            eprintln!("Skipping giant allocation test due to insufficient memory.");
            return;
        }

        let vec = vec![0u8; 5_000_000_000];

        let sequence: BytesView = vec.into();
        assert_eq!(sequence.len(), 5_000_000_000);
        assert_eq!(sequence.first_slice().len(), u32::MAX as usize);
        assert_eq!(sequence.into_spans_reversed().len(), 2);
    }

    #[test]
    fn test_vec_block_iterator_size_hint_single_block() {
        let vec = vec![b'H', b'e', b'l', b'l', b'o', b',', b' ', b'w', b'o', b'r', b'l', b'd', b'!'];
        let iterator = VecBlockIterator::new(vec);

        let (min, max) = iterator.size_hint();
        assert_eq!(min, 1);
        assert_eq!(max, Some(1));
    }

    #[test]
    fn test_vec_block_iterator_size_hint_multiple_blocks() {
        // Create a vec that requires exactly 2 blocks
        let size = (BlockSize::MAX as usize) + 1000;
        let vec = vec![0u8; size];

        let iterator = VecBlockIterator::new(vec);

        let (min, max) = iterator.size_hint();
        assert_eq!(min, 2);
        assert_eq!(max, Some(2));
    }

    #[test]
    fn test_vec_block_iterator_size_hint_empty() {
        let vec = Vec::new();
        let iterator = VecBlockIterator::new(vec);

        let (min, max) = iterator.size_hint();
        assert_eq!(min, 0);
        assert_eq!(max, Some(0));
    }

    #[test]
    fn test_vec_block_iterator_size_hint_exact_block_size() {
        // Create a vec that is exactly one block size
        let vec = vec![0u8; BlockSize::MAX as usize];

        let iterator = VecBlockIterator::new(vec);

        let (min, max) = iterator.size_hint();
        assert_eq!(min, 1);
        assert_eq!(max, Some(1));
    }
}
