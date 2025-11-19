// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::mem::MaybeUninit;
use std::num::NonZero;
use std::ptr::NonNull;

use crate::{BlockRef, SpanBuilder};

/// An integer type that can represent the size of a memory block in bytes.
///
/// Many operating system APIs are limited to 32-bit memory block sizes, so we match it.
///
/// A [`BytesView`][crate::BytesView] may contain more data than `BlockSize` by being composed of
/// multiple blocks but a single block is always limited to `BlockSize`.
pub type BlockSize = u32;

/// Describes an exclusively owned memory block that can be used as part of the capacity
/// of a newly created [`BytesBuf`][crate::BytesBuf].
#[derive(Debug)]
pub struct Block {
    ptr: NonNull<MaybeUninit<u8>>,

    len: NonZero<BlockSize>,

    /// As this type represents exclusive ownership, we know that this is the only `BlockRef` in
    /// existence that references this memory block.
    #[expect(clippy::struct_field_names, reason = "acceptable, block_ref is standard term in many places")]
    block_ref: BlockRef,
}

impl Block {
    /// Describes an exclusively owned memory block.
    ///
    /// # Safety
    ///
    /// The caller must guarantee exclusive ownership - nothing else may reference the capacity of
    /// this memory block until the last [`BlockRef`] from the same family of clones as `block_ref`
    /// is dropped.
    #[must_use]
    pub const unsafe fn new(ptr: NonNull<MaybeUninit<u8>>, len: NonZero<BlockSize>, block_ref: BlockRef) -> Self {
        Self { ptr, len, block_ref }
    }

    pub(crate) fn into_span_builder(self) -> SpanBuilder {
        // SAFETY: Our type safety invariant requires that the block is exclusively owned, which is
        // enforced by safety requirements on `new()`. This meets the downstream requirements.
        unsafe { SpanBuilder::new(self.ptr, self.len, self.block_ref) }
    }
}
