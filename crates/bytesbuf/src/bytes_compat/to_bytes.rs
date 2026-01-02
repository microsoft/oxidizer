// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::{Bytes, BytesMut};
use nm::Event;

use crate::BytesView;

impl BytesView {
    /// Returns a `bytes::Bytes` that contains the same byte sequence.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytes::Buf;
    /// use bytesbuf::BytesView;
    ///
    /// let view = BytesView::copied_from_slice(b"\x12\x34\x56\x78", &memory);
    ///
    /// let mut bytes = view.to_bytes();
    ///
    /// // Consume the data using the bytes crate's Buf trait.
    /// assert_eq!(bytes.get_u16(), 0x1234);
    /// assert_eq!(bytes.get_u16(), 0x5678);
    /// assert!(!bytes.has_remaining());
    /// ```
    ///
    /// # Performance
    ///
    /// This operation is zero-copy if the sequence is backed by a single consecutive
    /// slice of memory capacity.
    ///
    /// If the sequence is backed by multiple slices of memory capacity, the data will be copied
    /// to a new `Bytes` instance backed by new memory capacity from the Rust global allocator.
    ///
    /// **You generally want to avoid this conversion in performance-sensitive code.**
    ///
    /// This conversion always requires a small dynamic memory allocation for
    /// metadata, so avoiding conversions is valuable even if zero-copy.
    ///
    /// # Why is this not `.into()`?
    ///
    /// We do not allow conversion via `.into()` because the conversion is not guaranteed to be
    /// a cheap operation and may involve data copying. The `.to_bytes()` function must always
    /// be explicitly called to make the conversion more obvious and easier to catch in reviews.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn to_bytes(&self) -> Bytes {
        if self.spans_reversed.is_empty() {
            TO_BYTES_SHARED.with(|x| x.observe(0));

            Bytes::new()
        } else if self.spans_reversed.len() == 1 {
            // We are a single-span view, which can always be zero-copy represented.
            TO_BYTES_SHARED.with(|x| x.observe(self.len()));

            Bytes::from_owner(self.spans_reversed.first().expect("we verified there is one span").clone())
        } else {
            // We must copy, as Bytes can only represent consecutive spans of data.
            let mut bytes = BytesMut::with_capacity(self.len());

            for span in self.spans_reversed.iter().rev() {
                bytes.extend_from_slice(span);
            }

            debug_assert_eq!(self.len(), bytes.len());

            TO_BYTES_COPIED.with(|x| x.observe(self.len()));

            bytes.freeze()
        }
    }
}

thread_local! {
    static TO_BYTES_SHARED: Event = Event::builder()
        .name("bytesbuf_view_to_bytes_shared")
        .build();

    static TO_BYTES_COPIED: Event = Event::builder()
        .name("bytesbuf_view_to_bytes_copied")
        .build();
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use bytes::Buf;
    use new_zealand::nz;

    use super::*;
    use crate::mem::testing::{TransparentMemory, std_alloc_block};

    #[test]
    fn view_to_bytes() {
        let mut builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        builder.put_slice(&1234_u64.to_ne_bytes());
        builder.put_slice(&5678_u64.to_ne_bytes());

        let span1 = builder.consume(nz!(8));
        let span2 = builder.consume(nz!(8));

        let view_single_span = BytesView::from_spans(vec![span1.clone()]);
        let view_multi_span = BytesView::from_spans(vec![span1, span2]);

        let mut bytes = view_single_span.to_bytes();
        assert_eq!(8, bytes.len());
        assert_eq!(1234, bytes.get_u64_ne());

        let mut bytes = view_single_span.to_bytes();
        assert_eq!(8, bytes.len());
        assert_eq!(1234, bytes.get_u64_ne());

        let mut bytes = view_multi_span.to_bytes();
        assert_eq!(16, bytes.len());
        assert_eq!(1234, bytes.get_u64_ne());
        assert_eq!(5678, bytes.get_u64_ne());
    }

    #[test]
    fn empty_view_to_bytes() {
        let view = BytesView::default();
        let bytes = view.to_bytes();
        assert_eq!(0, bytes.len());
    }

    #[test]
    fn test_view_to_bytes() {
        let memory = TransparentMemory::new();

        let view = BytesView::copied_from_slice(b"Hello, world!", &memory);

        let view_chunk_ptr = view.first_slice().as_ptr();

        let bytes = view.to_bytes();

        assert_eq!(bytes.as_ref(), b"Hello, world!");

        // We expect this to be zero-copy since we used the passthrough allocator.
        assert_eq!(bytes.as_ptr(), view_chunk_ptr);
    }

    #[test]
    fn test_multi_block_view_to_bytes() {
        let memory = TransparentMemory::new();

        let hello = BytesView::copied_from_slice(b"Hello, ", &memory);
        let world = BytesView::copied_from_slice(b"world!", &memory);
        let view = BytesView::from_views([hello, world]);

        let bytes = view.to_bytes();
        assert_eq!(bytes.as_ref(), b"Hello, world!");
    }
}
