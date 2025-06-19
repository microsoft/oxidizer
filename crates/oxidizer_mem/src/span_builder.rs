// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::slice;
use std::num::NonZero;
use std::ops::Deref;
use std::sync::Arc;

use bytes::buf::UninitSlice;
use bytes::{Buf, BufMut};

use crate::{Block, Span};

/// Owns a mutable span of memory from an I/O block, which can be filled with data,
/// enabling you to detach immutable spans from the front to create views over the data.
///
/// Use the [`bytes::buf::BufMut`][1] implementation to fill available memory with data,
/// after which you may detach spans of immutable data from the front via [`consume()`][3].
///
/// Filled bytes may be inspected via [`inspect()`][4] to enable a content-based determination to be made
/// on whether (part of) the filled data is ready to be consumed.
///
/// [1]: https://docs.rs/bytes/latest/bytes/buf/trait.BufMut.html
/// [3]: Self::consume
/// [4]: Self::inspect
#[derive(Debug)]
pub struct SpanBuilder {
    block: Arc<Block>,

    // Start offset (how many bytes from the start of the block to ignore).
    // We ignore bytes that have already been detached from the front as immutable ranges.
    start_offset: usize,

    // Number of bytes after `start_offset` that have been filled with data.
    // Any bytes after this range must be treated as uninitialized.
    filled_bytes: usize,
}

impl SpanBuilder {
    pub(crate) const fn new(block: Arc<Block>) -> Self {
        Self {
            block,
            start_offset: 0,
            filled_bytes: 0,
        }
    }

    /// Number of bytes at the front that have been filled with data.
    pub const fn len(&self) -> usize {
        self.filled_bytes
    }

    pub const fn is_empty(&self) -> bool {
        self.filled_bytes == 0
    }

    pub const fn inspect(&self) -> InspectSpanBuilderData {
        InspectSpanBuilderData {
            builder: self,
            offset: self.start_offset,
            len: self.filled_bytes,
        }
    }

    /// Consumes the specified number of bytes (of already filled data) from the front of the
    /// builder's I/O block, returning a span with those immutable bytes.
    ///
    /// # Panics
    ///
    /// Panics iIf the requested number of bytes to return exceeds the number of bytes filled
    /// with data.
    pub fn consume(&mut self, len: NonZero<usize>) -> Span {
        self.consume_checked(len)
            .expect("attempted to consume more bytes than available in builder")
    }

    /// Consumes the specified number of bytes (of already filled data) from the front of the
    /// builder's I/O block, returning a span with those immutable bytes.
    ///
    /// Returns `None` if the requested number of bytes to return
    /// exceeds the number of bytes filled with data.
    pub fn consume_checked(&mut self, len: NonZero<usize>) -> Option<Span> {
        if len.get() > self.filled_bytes {
            return None;
        }

        // SAFETY: We must guarantee that the region has been initialized.
        // Yes, it has - we can see that from `filled_bytes`.
        let span = unsafe { Span::new(Arc::clone(&self.block), self.start_offset, len.get()) };

        self.start_offset = self
            .start_offset
            .checked_add(len.get())
            .expect("inconceivable to overflow usize here");

        self.filled_bytes = self
            .filled_bytes
            .checked_sub(len.get())
            .expect("already handled the case where len > filled_bytes");

        Some(span)
    }

    /// Allows the underlying memory block to be accessed, primarily used to extend its lifetime
    /// beyond that of the `SpanBuilder` itself.
    pub(crate) const fn block(&self) -> &Arc<Block> {
        &self.block
    }
}

// SAFETY: The trait does not clearly state any safety requirements we must satisfy, so it is
// unclear why this trait is marked unsafe. Cross your fingers and hope for the best!
unsafe impl BufMut for SpanBuilder {
    fn remaining_mut(&self) -> usize {
        self.block
            .size()
            .get()
            .checked_sub(self.start_offset)
            .expect("type invariant - start offset cannot be larger than block size")
            .checked_sub(self.filled_bytes)
            .expect("type invariant - remaining capacity cannot be negative")
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        self.filled_bytes = self
            .filled_bytes
            .checked_add(cnt)
            .expect("attempted to advance SpanBuilder across usize overflow");

        // Verify that type invariants remain valid.
        // This will panic if we advanced beyond the end of the block.
        _ = self.remaining_mut();
    }

    fn chunk_mut(&mut self) -> &mut bytes::buf::UninitSlice {
        let remaining_bytes = self.remaining_mut();

        let available_start_offset = self.start_offset.checked_add(self.filled_bytes).expect(
            "start offset + filled bytes is a tiny sum that cannot possibly overflow usize",
        );

        // SAFETY: We did the math, it checked out.
        let available_ptr = unsafe { self.block.as_ptr().add(available_start_offset).as_ptr() };

        // SAFETY: We are responsible for the pointer pointing to a valid storage of the given type
        // (guaranteed by I/O block) and for the slice having exclusive access to the memory for the
        // duration of its lifetime (guaranteed by `&mut self` which inherits exclusive access from
        // the SpanBuilder itself).
        let available_slice = unsafe { slice::from_raw_parts_mut(available_ptr, remaining_bytes) };

        UninitSlice::uninit(available_slice)
    }
}

#[derive(Debug)]
pub struct InspectSpanBuilderData<'a> {
    builder: &'a SpanBuilder,
    offset: usize,
    len: usize,
}

impl Deref for InspectSpanBuilderData<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        // SAFETY: We must guarantee that the region has been initialized.
        // Our range is covered by SpanBuilder::filled_bytes, which ensures that.
        let ptr = unsafe {
            self.builder
                .block
                .as_ptr()
                .add(self.offset)
                .as_ptr()
                .cast::<u8>()
        };

        // SAFETY: We are responsible for the pointer pointing to a valid storage of the given type
        // (guaranteed by I/O block) and for there not being any mutation of the memory for the
        // duration of the slice's lifetime (guaranteed by borrowing from the SpanBuilder).
        unsafe { slice::from_raw_parts(ptr, self.len) }
    }
}

impl Buf for InspectSpanBuilderData<'_> {
    fn remaining(&self) -> usize {
        self.len
    }

    #[cfg_attr(test, mutants::skip)] // Mutations can cause infinite loops in tests.
    fn chunk(&self) -> &[u8] {
        self
    }

    fn advance(&mut self, cnt: usize) {
        self.len = self
            .len
            .checked_sub(cnt)
            .expect("cannot advance inspection window past the end of the SpanBuilder");

        self.offset = self
            .offset
            .checked_add(cnt)
            .expect("overflow of usize is inconceivable here");
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::testing::assert_panic;

    #[test]
    fn smoke_test() {
        let block = Block::new(NonZero::new(10).unwrap());

        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let mut builder = unsafe { block.take_ownership() };

        assert_eq!(builder.len(), 0);
        assert!(builder.is_empty());
        assert_eq!(builder.remaining_mut(), 10);

        assert!(builder.consume_checked(NonZero::new(1).unwrap()).is_none());

        builder.put_u64(1234);

        assert_eq!(builder.len(), 8);
        assert!(!builder.is_empty());
        assert_eq!(builder.remaining_mut(), 2);

        _ = builder.consume(NonZero::new(8).unwrap());

        assert_eq!(builder.len(), 0);
        assert!(builder.is_empty());
        assert_eq!(builder.remaining_mut(), 2);

        builder.put_u16(1234);

        assert_eq!(builder.len(), 2);
        assert!(!builder.is_empty());
        assert_eq!(builder.remaining_mut(), 0);

        _ = builder.consume(NonZero::new(2).unwrap());

        assert_eq!(builder.len(), 0);
        assert!(builder.is_empty());
        assert_eq!(builder.remaining_mut(), 0);
    }

    #[test]
    fn inspect() {
        let block = Block::new(NonZero::new(10).unwrap());

        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let mut builder = unsafe { block.take_ownership() };

        builder.put_u32(1234);
        builder.put_u32(5678);
        builder.put_u16(90);

        assert_eq!(builder.len(), 10);
        assert_eq!(builder.remaining_mut(), 0);

        let mut inspector = builder.inspect();

        assert_eq!(inspector.remaining(), 10);
        assert_eq!(inspector.chunk().len(), 10);

        assert_eq!(inspector.get_u32(), 1234);
        assert_eq!(inspector.get_u32(), 5678);
        assert_eq!(inspector.get_u16(), 90);

        assert_eq!(inspector.remaining(), 0);
        assert_eq!(inspector.chunk().len(), 0);

        assert_eq!(builder.len(), 10);
        assert_eq!(builder.remaining_mut(), 0);

        _ = builder.consume(NonZero::new(10).unwrap());

        assert_eq!(builder.len(), 0);
        assert_eq!(builder.remaining_mut(), 0);
    }

    #[test]
    fn append_oob_panics() {
        let block = Block::new(NonZero::new(10).unwrap());

        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let mut builder = unsafe { block.take_ownership() };

        builder.put_u32(1234);
        builder.put_u32(5678);
        assert_panic!(builder.put_u32(90)); // Tries to append 4 when only 2 bytes available.
    }

    #[test]
    fn inspect_oob_panics() {
        let block = Block::new(NonZero::new(10).unwrap());

        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let mut builder = unsafe { block.take_ownership() };

        builder.put_u32(1234);
        builder.put_u32(5678);
        builder.put_u16(90);

        let mut inspector = builder.inspect();
        assert_eq!(inspector.get_u32(), 1234);
        assert_eq!(inspector.get_u32(), 5678);
        assert_panic!(_ = inspector.get_u32()); // Tries to read 4 when only 2 bytes remaining.
    }

    #[test]
    fn thread_safe_type() {
        assert_impl_all!(SpanBuilder: Send, Sync);
    }
}