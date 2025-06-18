// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::{Deref, Range};
use std::slice;
use std::sync::Arc;

use bytes::{Buf, Bytes};

use crate::Block;

/// A span of immutable bytes backed by memory from an I/O block. This type is used as a building
/// block for [`Sequence`]s, which are used for carrying data into or out of I/O operations.
///
/// While the contents are immutable, the span itself is not - its size may be constrained by
/// cutting off pieces from the front (consuming them) as the read cursor is advanced when calling
/// members of the [`bytes::Buf][1]` trait implementation.
///
/// Contents that have been consumed are no longer considered part of the span (any remaining
/// content shifts to index 0 after data is consumed from the front).
///
/// Sub-slices of a span may be formed by calling [`slice()`][3]. This does not copy the data,
/// merely creates a new and independent view over the same immutable memory.
#[derive(Debug)]
pub struct Span {
    block: Arc<Block>,

    offset: usize,
    len: usize,
}

impl Span {
    /// # Safety
    ///
    /// The caller must guarantee that the referenced region of memory has been initialized.
    pub(crate) unsafe fn new(block: Arc<Block>, offset: usize, len: usize) -> Self {
        assert!(offset.saturating_add(len) <= block.size().get());

        Self { block, offset, len }
    }

    /// Returns a sub-slice of the span.
    ///
    /// The bounds logic only considers data currently present in the span.
    /// Any data already consumed is not considered part of the span.
    ///
    /// # Panics
    ///
    /// Panics if the provided range is outside the bounds of the span.
    pub(crate) fn slice(&self, range: Range<usize>) -> Self {
        self.slice_checked(range)
            .expect("provided range out of span bounds")
    }

    /// Returns a sub-slice of the span or `None` if the range is outside the bounds of the span.
    ///
    /// The bounds logic only considers data currently present in the span.
    /// Any data already consumed is not considered part of the span.
    pub(crate) fn slice_checked(&self, range: Range<usize>) -> Option<Self> {
        let start = self.offset.saturating_add(range.start);
        let end = self.offset.saturating_add(range.end);

        (end <= self.offset.saturating_add(self.len)).then_some(Self {
            block: Clone::clone(&self.block),
            offset: start,
            len: range.len(),
        })
    }

    /// Allows the underlying memory block to be accessed, primarily used to extend its lifetime
    /// beyond that of the `Span` itself.
    pub(crate) const fn block(&self) -> &Arc<Block> {
        &self.block
    }

    /// Returns the span as an instance of `Bytes`. This operation is zero-copy.
    pub(crate) fn to_bytes(&self) -> Bytes {
        Bytes::from_owner(self.clone())
    }
}

impl Clone for Span {
    fn clone(&self) -> Self {
        Self {
            block: Arc::clone(&self.block),
            offset: self.offset,
            len: self.len,
        }
    }
}

impl Deref for Span {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        // SAFETY: We must guarantee that the region has been initialized.
        // This is always true for a Span - all the memory it references is
        // initialized, as guaranteed by the SpanMut that created it.
        let ptr = unsafe { self.block.as_ptr().add(self.offset).as_ptr().cast::<u8>() };

        // SAFETY: We are responsible for the pointer pointing to a valid storage of the given type
        // (guaranteed by I/O block) and for there not being any mutation of the memory for the
        // duration of the slice's lifetime (guaranteed by SpanBuilder which prevents further
        // mutation of any part of the I/O block that it has released to a Span).
        unsafe { slice::from_raw_parts(ptr, self.len) }
    }
}

impl AsRef<[u8]> for Span {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl Buf for Span {
    #[cfg_attr(test, mutants::skip)] // Mutating this can cause infinite loops.
    fn remaining(&self) -> usize {
        self.len
    }

    #[cfg_attr(test, mutants::skip)] // Mutating this can cause infinite loops.
    fn chunk(&self) -> &[u8] {
        self
    }

    fn advance(&mut self, cnt: usize) {
        self.len = self
            .len
            .checked_sub(cnt)
            .expect("cannot advance span past the end");

        self.offset = self
            .offset
            .checked_add(cnt)
            .expect("overflow of usize is inconceivable here");
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZero;

    use bytes::BufMut;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::testing::assert_panic;

    #[test]
    fn smoke_test() {
        let block = Block::new(NonZero::new(10).unwrap());

        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let mut builder = unsafe { block.take_ownership() };

        builder.put_u64(1234);
        builder.put_u16(16);

        let mut span = builder.consume(NonZero::new(10).unwrap());

        assert_eq!(0, builder.remaining_mut());
        assert_eq!(span.remaining(), 10);

        let slice = span.chunk();
        assert_eq!(10, slice.len());

        assert_eq!(span.get_u64(), 1234);
        assert_eq!(span.get_u16(), 16);

        assert_eq!(0, span.remaining());
    }

    #[test]
    fn oob_is_panic() {
        let block = Block::new(NonZero::new(10).unwrap());

        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let mut builder = unsafe { block.take_ownership() };

        builder.put_u64(1234);
        builder.put_u16(16);

        let mut span = builder.consume(NonZero::new(10).unwrap());

        assert_eq!(span.get_u64(), 1234);
        assert_panic!(_ = span.get_u32()); // Reads 4 but only has 2 remaining.
    }

    #[test]
    fn slice() {
        let block = Block::new(NonZero::new(10).unwrap());

        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let mut builder = unsafe { block.take_ownership() };

        builder.put_u64(1234);
        builder.put_u16(16);

        let mut span1 = builder.consume(NonZero::new(10).unwrap());
        let mut span2 = span1.slice(0..10);
        let mut span3 = span1.slice(8..10);
        let span4 = span1.slice(10..10);
        assert!(span1.slice_checked(0..11).is_none()); // Out of bounds.

        assert_eq!(span1.remaining(), 10);
        assert_eq!(span2.remaining(), 10);
        assert_eq!(span3.remaining(), 2);
        assert_eq!(span4.remaining(), 0);

        assert_eq!(span1.get_u64(), 1234);
        assert_eq!(span1.get_u16(), 16);

        assert_eq!(span2.get_u64(), 1234);
        assert_eq!(span2.get_u16(), 16);

        assert_eq!(span3.get_u16(), 16);
    }

    #[test]
    fn thread_safe_type() {
        assert_impl_all!(Span: Send, Sync);
    }
}