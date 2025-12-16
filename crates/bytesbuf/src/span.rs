// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ops::{Bound, Deref, RangeBounds};
use std::ptr::NonNull;
use std::{fmt, slice};

use crate::{BlockRef, BlockSize};

/// A span of immutable bytes backed by memory from a memory block.
///
/// This type is used as a building block for [`BytesView`][crate::BytesView].
///
/// While the contents are immutable, the span itself is not - its size may be constrained by
/// cutting off bytes from the front (i.e. consuming them).
///
/// Contents that have been consumed are no longer considered part of the span (any remaining
/// content shifts to index 0).
///
/// Sub-slices of a span may be formed by calling [`.slice()`]. This does not copy the data,
/// merely creates a new and independent view over the same immutable bytes.
///
/// # Ownership of memory blocks
///
/// See [`SpanBuilder`][crate::SpanBuilder] for details.
#[derive(Clone)]
pub(crate) struct Span {
    // For the purposes of the `Span` and `SpanBuilder` types, this merely controls the lifecycle
    // of the memory block - dropping the last reference will permit the memory block to be
    // reclaimed by the memory provider it originates from.
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
    pub(crate) const fn is_empty(&self) -> bool {
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

    /// References the memory block that provides the span's memory capacity.
    pub(crate) const fn block_ref(&self) -> &BlockRef {
        &self.block_ref
    }

    /// Marks `len` bytes as consumed from the start of the span, shrinking it.
    ///
    /// The remaining bytes shift to index 0.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `len` is less than or equal to the current length of the span.
    pub(crate) unsafe fn advance(&mut self, len: usize) {
        #[expect(clippy::cast_possible_truncation, reason = "guaranteed by safety requirements")]
        let len_bs = len as BlockSize;

        // Will never wrap - guaranteed by safety requirements.
        self.len = self.len.wrapping_sub(len_bs);

        // SAFETY: Guaranteed by safety requirements.
        self.start = unsafe { self.start.add(len) };
    }

    /// Testing helper for easily consuming a fixed number of bytes from the front.
    #[cfg(test)]
    pub(crate) fn get_array<const N: usize>(&mut self) -> [u8; N] {
        assert!(self.len() >= N as BlockSize, "out of bounds read");

        let mut array = [0_u8; N];

        // SAFETY: Assertion above guarantees that we have enough bytes of data in the span.
        let src = unsafe { slice::from_raw_parts(self.start.as_ptr(), N) };

        array.copy_from_slice(src);

        // SAFETY: Guarded by assertion above.
        unsafe {
            self.advance(N);
        }

        array
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

impl fmt::Debug for Span {
    #[cfg_attr(coverage_nightly, coverage(off))] // There is no specific API contract here for us to test.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("Span");

        debug_struct.field("start", &self.start).field("len", &self.len);

        if self.len >= 4 {
            let byte1 = self[0];
            let byte2 = self[1];
            let byte3 = self[2];
            let byte4 = self[3];

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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::num::NonZero;

    use new_zealand::nz;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::std_alloc_block;

    // The type is thread-mobile (Send) and can be shared (for reads) between threads (Sync).
    assert_impl_all!(Span: Send, Sync);

    #[test]
    fn smoke_test() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        builder.put_slice(&1234_u64.to_ne_bytes());
        builder.put_slice(&16_u16.to_ne_bytes());

        let mut span = builder.consume(nz!(10));

        assert_eq!(0, builder.remaining_capacity());
        assert_eq!(span.len(), 10);
        assert!(!span.is_empty());

        assert_eq!(10, span.as_ref().len());

        assert_eq!(u64::from_ne_bytes(span.get_array()), 1234);
        assert_eq!(u16::from_ne_bytes(span.get_array()), 16);

        assert_eq!(0, span.len());
        assert!(span.is_empty());
    }

    #[test]
    fn slice() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        builder.put_slice(&1234_u64.to_ne_bytes());
        builder.put_slice(&16_u16.to_ne_bytes());

        let mut span1 = builder.consume(nz!(10));
        let mut span2 = span1.slice(0..10);
        let mut span3 = span1.slice(8..10);
        let span4 = span1.slice(10..10);
        assert!(span1.slice_checked(0..11).is_none()); // Out of bounds.

        assert_eq!(span1.len(), 10);
        assert_eq!(span2.len(), 10);
        assert_eq!(span3.len(), 2);
        assert_eq!(span4.len(), 0);

        assert_eq!(u64::from_ne_bytes(span1.get_array()), 1234);
        assert_eq!(u16::from_ne_bytes(span1.get_array()), 16);

        assert_eq!(u64::from_ne_bytes(span2.get_array()), 1234);
        assert_eq!(u16::from_ne_bytes(span2.get_array()), 16);
        assert_eq!(u16::from_ne_bytes(span3.get_array()), 16);
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

        sb.put_slice(&[0, 1, 2, 3, 4, 5]);

        let span = sb.consume(NonZero::new(sb.len()).unwrap());

        let mut middle_four = span.slice(1..5);
        assert_eq!(4, middle_four.len());
        assert_eq!(1, u8::from_ne_bytes(middle_four.get_array()));
        assert_eq!(2, u8::from_ne_bytes(middle_four.get_array()));
        assert_eq!(3, u8::from_ne_bytes(middle_four.get_array()));
        assert_eq!(4, u8::from_ne_bytes(middle_four.get_array()));

        let mut middle_four = span.slice(1..=4);
        assert_eq!(4, middle_four.len());
        assert_eq!(1, u8::from_ne_bytes(middle_four.get_array()));
        assert_eq!(2, u8::from_ne_bytes(middle_four.get_array()));
        assert_eq!(3, u8::from_ne_bytes(middle_four.get_array()));
        assert_eq!(4, u8::from_ne_bytes(middle_four.get_array()));

        let mut last_two = span.slice(4..);
        assert_eq!(2, last_two.len());
        assert_eq!(4, u8::from_ne_bytes(last_two.get_array()));
        assert_eq!(5, u8::from_ne_bytes(last_two.get_array()));

        let mut first_two = span.slice(..2);
        assert_eq!(2, first_two.len());
        assert_eq!(0, u8::from_ne_bytes(first_two.get_array()));
        assert_eq!(1, u8::from_ne_bytes(first_two.get_array()));

        let mut first_two = span.slice(..=1);
        assert_eq!(2, first_two.len());
        assert_eq!(0, u8::from_ne_bytes(first_two.get_array()));
        assert_eq!(1, u8::from_ne_bytes(first_two.get_array()));
    }

    #[test]
    fn slice_checked_with_excluded_start_bound() {
        use std::ops::Bound;

        let mut sb = std_alloc_block::allocate(nz!(100)).into_span_builder();

        sb.put_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8]);

        let span = sb.consume(NonZero::new(sb.len()).unwrap());

        // Test with excluded start bound: (Bound::Excluded(1), Bound::Excluded(5))
        // This should be equivalent to 2..5 (items at indices 2, 3, 4)
        let sliced = span.slice_checked((Bound::Excluded(1), Bound::Excluded(5)));
        assert!(sliced.is_some());
        let mut sliced = sliced.unwrap();
        assert_eq!(3, sliced.len());
        assert_eq!(2, u8::from_ne_bytes(sliced.get_array()));
        assert_eq!(3, u8::from_ne_bytes(sliced.get_array()));
        assert_eq!(4, u8::from_ne_bytes(sliced.get_array()));

        // Test edge case: excluded start at the last valid index returns empty sequence
        let sliced = span.slice_checked((Bound::Excluded(8), Bound::Unbounded));
        assert!(sliced.is_some());
        assert_eq!(0, sliced.unwrap().len());
    }
}
