// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::{Bytes, BytesMut};
#[cfg(feature = "std")]
use nm::Event;

use crate::BytesView;

impl BytesView {
    /// Returns a `bytes::Bytes` that contains the same byte sequence.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
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
    /// # }
    /// # }
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
    /// # Why prefer `.to_bytes()` over `.into()`?
    ///
    /// While conversion via `.into()` is supported, prefer calling `.to_bytes()` explicitly
    /// because the conversion is not guaranteed to be a cheap operation and may involve data
    /// copying. An explicit `.to_bytes()` call makes the conversion more obvious and easier
    /// to catch in reviews.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn to_bytes(&self) -> Bytes {
        if self.spans_reversed.is_empty() {
            #[cfg(feature = "std")]
            TO_BYTES_SHARED.with(|x| x.observe(0));

            Bytes::new()
        } else if self.spans_reversed.len() == 1 {
            // We are a single-span view, which can always be zero-copy represented.
            #[cfg(feature = "std")]
            TO_BYTES_SHARED.with(|x| x.observe(self.len()));

            Bytes::from_owner(self.spans_reversed.first().expect("we verified there is one span").clone())
        } else {
            // We must copy, as Bytes can only represent consecutive spans of data.
            let mut bytes = BytesMut::with_capacity(self.len());

            for span in self.spans_reversed.iter().rev() {
                bytes.extend_from_slice(span);
            }

            debug_assert_eq!(self.len(), bytes.len());

            #[cfg(feature = "std")]
            TO_BYTES_COPIED.with(|x| x.observe(self.len()));

            bytes.freeze()
        }
    }
}

impl From<BytesView> for Bytes {
    fn from(value: BytesView) -> Self {
        value.to_bytes()
    }
}

#[cfg(feature = "std")]
std::thread_local! {
    static TO_BYTES_SHARED: Event = Event::builder()
        .name("bytesbuf_view_to_bytes_shared")
        .build();

    static TO_BYTES_COPIED: Event = Event::builder()
        .name("bytesbuf_view_to_bytes_copied")
        .build();
}
