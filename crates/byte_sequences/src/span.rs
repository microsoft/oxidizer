// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::{Bound, Deref, RangeBounds};
use std::ptr::NonNull;
use std::{fmt, slice};

use bytes::{Buf, Bytes};

use crate::{BlockRef, BlockSize};

/// A span of immutable bytes backed by memory from a memory block. This type is used as a building
/// block for [`Sequence`][crate::Sequence]s.
///
/// While the contents are immutable, the span itself is not - its size may be constrained by
/// cutting off pieces from the front (consuming them) as the read cursor is advanced when calling
/// members of the [`bytes::Buf][1]` trait implementation.
///
/// Contents that have been consumed are no longer considered part of the span (any remaining
/// content shifts to index 0 after data is consumed from the front).
///
/// Sub-slices of a span may be formed by calling [`.slice()`]. This does not copy the data,
/// merely creates a new and independent view over the same immutable memory.
#[derive(Clone)]
pub struct Span {
    block_ref: BlockRef,

    start: NonNull<u8>,
    len: BlockSize,
}

impl Span {
    /// # Safety
    ///
    /// The caller must guarantee that the memory block region referenced by (start, len)
    /// has been initialized.
    pub(crate) const unsafe fn new(start: NonNull<u8>, len: BlockSize, block_ref: BlockRef) -> Self {
        Self { block_ref, start, len }
    }

    #[must_use]
    pub const fn len(&self) -> BlockSize {
        self.len
    }

    #[must_use]
    #[cfg(test)] // Not required elsewhere for now but if we need it, enable it.
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a sub-slice of the span.
    ///
    /// The bounds logic only considers data currently present in the span.
    /// Any data already consumed is not considered part of the span.
    ///
    /// # Panics
    ///
    /// Panics if the provided range is outside the bounds of the span.
    pub(crate) fn slice<R>(&self, range: R) -> Self
    where
        R: RangeBounds<BlockSize>,
    {
        self.slice_checked(range).expect("provided range is out of span bounds")
    }

    /// Returns a sub-slice of the span or `None` if the range is outside the bounds of the span.
    ///
    /// The bounds logic only considers data currently present in the span.
    /// Any data already consumed is not considered part of the span.
    pub(crate) fn slice_checked<R>(&self, range: R) -> Option<Self>
    where
        R: RangeBounds<BlockSize>,
    {
        let bytes_until_range = match range.start_bound() {
            Bound::Included(&x) => x,
            Bound::Excluded(&x) => x.checked_add(1)?,
            Bound::Unbounded => 0,
        };

        let bytes_in_range = match range.end_bound() {
            Bound::Included(&x) => x.checked_add(1)?.checked_sub(bytes_until_range)?,
            Bound::Excluded(&x) => x.checked_sub(bytes_until_range)?,
            Bound::Unbounded => self.len().checked_sub(bytes_until_range)?,
        };

        // Correctness guaranteed by `range` already being bounded to BlockSize.
        let required_len = bytes_until_range.wrapping_add(bytes_in_range);

        if required_len > self.len {
            // Not enough data to cover the range.
            return None;
        }

        Some(Self {
            block_ref: Clone::clone(&self.block_ref),
            // SAFETY: We validate above that the range is in-bounds of the span, so all is well.
            start: unsafe { self.start.add(bytes_until_range as usize) },
            len: bytes_in_range,
        })
    }

    /// Allows the underlying memory block to be accessed, primarily used to extend its lifetime
    /// beyond that of the `Span` itself.
    pub(crate) const fn block_ref(&self) -> &BlockRef {
        &self.block_ref
    }

    /// Returns the span as an instance of `Bytes`. This operation is zero-copy.
    pub(crate) fn to_bytes(&self) -> Bytes {
        Bytes::from_owner(self.clone())
    }
}

impl Deref for Span {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        // SAFETY: We are responsible for the pointer pointing to a valid storage of the given type
        // (guaranteed by memory block) and for there not being any mutation of the memory for the
        // duration of the slice's lifetime (guaranteed by SpanBuilder which prevents further
        // mutation of any part of the memory block that it has released to a Span).
        unsafe { slice::from_raw_parts(self.start.as_ptr(), self.len as usize) }
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
        self.len as usize
    }

    #[cfg_attr(test, mutants::skip)] // Mutating this can cause infinite loops.
    fn chunk(&self) -> &[u8] {
        self
    }

    fn advance(&mut self, cnt: usize) {
        // If it does not fit into BlockSize, it for sure does not fit in the block.
        let count: BlockSize = BlockSize::try_from(cnt).expect("attempted to advance past end of span");

        // Length is subtracted first, so even if we panic later, we do not overshoot the block.
        self.len = self.len.checked_sub(count).expect("attempted to advance past end of span");

        // SAFETY: We validated above that the pointer remains in-bounds.
        self.start = unsafe { self.start.add(count as usize) };
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("Span");

        debug_struct.field("start", &self.start).field("len", &self.len);

        if self.len >= 4 {
            let mut clone = self.clone();
            let byte1 = clone.get_u8();
            let byte2 = clone.get_u8();
            let byte3 = clone.get_u8();
            let byte4 = clone.get_u8();

            debug_struct.field("first_four_bytes", &format!("{byte1:02x}{byte2:02x}{byte3:02x}{byte4:02x}"));
        }

        debug_struct.field("block_ref", &self.block_ref).finish()
    }
}

// SAFETY: The presence of pointers disables Send but we re-enable it here because all our internal
// state is thread-mobile.
unsafe impl Send for Span {}
// SAFETY: The presence of pointers disables Sync but we re-enable it here because all our internal
// state is thread-safe. Furthermore, instances are immutable so sharing is natural.
unsafe impl Sync for Span {}

#[cfg(test)]
mod tests {
    use bytes::BufMut;
    use new_zealand::nz;
    use static_assertions::assert_impl_all;
    use std::num::NonZero;
    use testing_aids::assert_panic;

    use super::*;
    use crate::std_alloc_block;

    #[test]
    fn smoke_test() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        builder.put_u64(1234);
        builder.put_u16(16);

        let mut span = builder.consume(nz!(10));

        assert_eq!(0, builder.remaining_mut());
        assert_eq!(span.remaining(), 10);
        assert!(!span.is_empty());

        let slice = span.chunk();
        assert_eq!(10, slice.len());

        assert_eq!(span.get_u64(), 1234);
        assert_eq!(span.get_u16(), 16);

        assert_eq!(0, span.remaining());
        assert!(span.is_empty());
    }

    #[test]
    fn oob_is_panic() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        builder.put_u64(1234);
        builder.put_u16(16);

        let mut span = builder.consume(nz!(10));

        assert_eq!(span.get_u64(), 1234);
        assert_panic!(_ = span.get_u32()); // Reads 4 but only has 2 remaining.
    }

    #[test]
    fn slice() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        builder.put_u64(1234);
        builder.put_u16(16);

        let mut span1 = builder.consume(nz!(10));
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
        // The type is thread-mobile (Send) and can be shared (for reads) between threads (Sync).
        assert_impl_all!(Span: Send, Sync);
    }

    #[test]
    fn size_change_detector() {
        // The point of this is not to say that we expect it to have a specific size but to allow
        // us to easily detect when the size changes and (if we choose to) bless the change.
        // We assume 64-bit pointers - any support for 32-bit is problem for the future.
        assert_eq!(size_of::<Span>(), 32);
    }

    #[test]
    fn slice_indexing_kinds() {
        let mut sb = std_alloc_block::allocate(nz!(10)).into_span_builder();

        sb.put_u8(0);
        sb.put_u8(1);
        sb.put_u8(2);
        sb.put_u8(3);
        sb.put_u8(4);
        sb.put_u8(5);

        let span = sb.consume(NonZero::new(sb.len()).unwrap());

        let mut middle_four = span.slice(1..5);
        assert_eq!(4, middle_four.len());
        assert_eq!(1, middle_four.get_u8());
        assert_eq!(2, middle_four.get_u8());
        assert_eq!(3, middle_four.get_u8());
        assert_eq!(4, middle_four.get_u8());

        let mut middle_four = span.slice(1..=4);
        assert_eq!(4, middle_four.len());
        assert_eq!(1, middle_four.get_u8());
        assert_eq!(2, middle_four.get_u8());
        assert_eq!(3, middle_four.get_u8());
        assert_eq!(4, middle_four.get_u8());

        let mut last_two = span.slice(4..);
        assert_eq!(2, last_two.len());
        assert_eq!(4, last_two.get_u8());
        assert_eq!(5, last_two.get_u8());

        let mut first_two = span.slice(..2);
        assert_eq!(2, first_two.len());
        assert_eq!(0, first_two.get_u8());
        assert_eq!(1, first_two.get_u8());

        let mut first_two = span.slice(..=1);
        assert_eq!(2, first_two.len());
        assert_eq!(0, first_two.get_u8());
        assert_eq!(1, first_two.get_u8());
    }

    #[test]
    fn slice_checked_with_excluded_start_bound() {
        use std::ops::Bound;

        let mut sb = std_alloc_block::allocate(nz!(100)).into_span_builder();

        sb.put_u8(0);
        sb.put_u8(1);
        sb.put_u8(2);
        sb.put_u8(3);
        sb.put_u8(4);
        sb.put_u8(5);
        sb.put_u8(6);
        sb.put_u8(7);
        sb.put_u8(8);

        let span = sb.consume(NonZero::new(sb.len()).unwrap());

        // Test with excluded start bound: (Bound::Excluded(1), Bound::Excluded(5))
        // This should be equivalent to 2..5 (items at indices 2, 3, 4)
        let sliced = span.slice_checked((Bound::Excluded(1), Bound::Excluded(5)));
        assert!(sliced.is_some());
        let mut sliced = sliced.unwrap();
        assert_eq!(3, sliced.len());
        assert_eq!(2, sliced.get_u8());
        assert_eq!(3, sliced.get_u8());
        assert_eq!(4, sliced.get_u8());

        // Test edge case: excluded start at the last valid index returns empty sequence
        let sliced = span.slice_checked((Bound::Excluded(8), Bound::Unbounded));
        assert!(sliced.is_some());
        assert_eq!(0, sliced.unwrap().len());
    }

    #[test]
    fn debug_includes_first_four_bytes_for_long_spans() {
        let mut builder = std_alloc_block::allocate(nz!(8)).into_span_builder();

        for byte in [0x10_u8, 0x20, 0x30, 0x40, 0x50] {
            builder.put_u8(byte);
        }

        let span = builder.consume(nz!(5));
        let debug = format!("{span:?}");

        assert!(debug.contains("first_four_bytes"), "expected preview field in {debug}");
        assert!(debug.contains("10203040"), "expected first four bytes preview in {debug}");
    }

    #[test]
    fn debug_omits_preview_for_short_spans() {
        let mut builder = std_alloc_block::allocate(nz!(3)).into_span_builder();

        builder.put_u8(0xaa);
        builder.put_u8(0xbb);
        builder.put_u8(0xcc);

        let span = builder.consume(nz!(3));
        let debug = format!("{span:?}");

        assert!(debug.contains("Span"));
        assert!(!debug.contains("first_four_bytes"), "unexpected preview field in {debug}");
    }
}
