// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;
use std::ptr::NonNull;

use crate::mem::{Block, BlockRef, BlockRefDynamic, BlockRefVTable, BlockSize};
use crate::{BytesBuf, BytesView};

impl From<&'static [u8]> for BytesView {
    fn from(value: &'static [u8]) -> Self {
        assert!(
            value.len() <= BlockSize::MAX as usize,
            "slice length exceeds BlockSize - static data of such enormous size is not supported"
        );

        #[expect(clippy::cast_possible_truncation, reason = "guarded by assertion above")]
        let Some(len) = NonZero::new(value.len() as BlockSize) else {
            return Self::new();
        };

        // SAFETY: A reference is never null.
        let universal_block_state = unsafe { NonNull::new_unchecked((&raw const UNIVERSAL_BLOCK_STATE).cast_mut()) };

        // SAFETY: The state must remain valid for the lifetime for the BlockRef. Well, our state is a no-op
        // placeholder that is never accessed, so it is a completely meaningless type, in fact, only existing
        // to satisfy the API contract but with entirely no-op functions inside.
        let block_ref = unsafe { BlockRef::new(universal_block_state, &BLOCK_REF_FNS) };

        // SAFETY: We started from a reference, which cannot possibly be null.
        let ptr = unsafe { NonNull::new(value.as_ptr().cast_mut()).unwrap_unchecked() };

        // SAFETY: Block requires us to guarantee exclusive access. We actually cannot do that - this
        // memory block is shared and immutable, unlike many others! However, the good news is that this
        // requirement on Block exists to support mutation. As long as we never treat the block as
        // having mutable contents, we are fine with shared immutable access.
        let block = unsafe { Block::new(ptr.cast(), len, block_ref) };

        let mut buf = BytesBuf::from_blocks([block]);

        // SAFETY: We know that the data is already initialized; we simply declare this to the
        // BytesBuf and get it to emit a completed BytesView from all its contents.
        unsafe {
            buf.advance(len.get() as usize);
        }

        buf.consume_all()
    }
}

impl<const LEN: usize> From<&'static [u8; LEN]> for BytesView {
    fn from(value: &'static [u8; LEN]) -> Self {
        value.as_slice().into()
    }
}

/// An implementation of `BlockRef` that is associated with a `&'static [u8]`.
///
/// As the state is empty, every instance can reuse it.
struct StaticSliceBlock;

static UNIVERSAL_BLOCK_STATE: StaticSliceBlock = StaticSliceBlock;

// SAFETY: We must guarantee thread-safety. We do.
unsafe impl BlockRefDynamic for StaticSliceBlock {
    type State = Self;

    #[cfg_attr(test, mutants::skip)] // Impractical to test - it does nothing, really.
    fn clone(_state_ptr: NonNull<Self::State>) -> NonNull<Self::State> {
        // SAFETY: A reference is never null.
        unsafe { NonNull::new_unchecked((&raw const UNIVERSAL_BLOCK_STATE).cast_mut()) }
    }

    #[cfg_attr(test, mutants::skip)] // Impractical to test. Miri will inform about memory leaks.
    fn drop(_state_ptr: NonNull<Self::State>) {}
}

const BLOCK_REF_FNS: BlockRefVTable<StaticSliceBlock> = BlockRefVTable::from_trait();

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_slice() {
        let data: &'static [u8] = b"hello";

        let seq = BytesView::from(data);

        assert_eq!(seq, data);
    }

    #[test]
    fn from_array() {
        let data = b"world";

        let seq = BytesView::from(data);

        assert_eq!(seq, data);
    }

    #[test]
    fn zero_sized_slice() {
        let data: &'static [u8] = b"";
        let seq = BytesView::from(data);

        assert_eq!(seq.len(), 0);
        assert!(seq.is_empty());
    }
}
