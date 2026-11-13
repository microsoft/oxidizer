// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(not(test))]
use alloc::vec::Vec;
use core::hash::{Hash, Hasher};
use core::iter;
use core::num::NonZero;
use core::ops::{Bound, RangeBounds};

#[cfg(feature = "std")]
use nm::{Event, Magnitude};
use smallvec::SmallVec;

use crate::mem::{BlockMeta, BlockSize, Memory};
use crate::telemetry::BufferIdentity;
use crate::{MAX_INLINE_SPANS, MemoryGuard, Span};

/// A view over a sequence of immutable bytes.
///
/// Only the contents are immutable - the view itself can be mutated in terms of progressively
/// marking the byte sequence as consumed until the view becomes empty.
///
/// # Creating a `BytesView`
///
/// Instances can be created in different ways:
///
/// * Use a [`BytesBuf`] to build the byte sequence piece by piece, consuming the buffered data as a new `BytesView` when finished.
/// * Clone an existing `BytesView` with [`clone()`].
/// * Take a range of bytes from an existing `BytesView` via [`range()`].
/// * Combine multiple `BytesView` instances into one via [`from_views()`], [`concat()`] or [`append()`].
/// * Copy data from a `&[u8]` using [`copied_from_slice()`].
///
/// Some of these methods may require you to first [obtain access to a memory provider].
#[doc = include_str!("../docs/snippets/sequence_memory_layout.md")]
///
/// # Example
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "std")] {
/// # let memory = bytesbuf::mem::GlobalPool::new();
/// use bytesbuf::BytesView;
///
/// let mut view = BytesView::copied_from_slice(b"Hello!", &memory);
///
/// // Read bytes one at a time until the view is empty.
/// while !view.is_empty() {
///     let byte = view.get_byte();
///     println!("Read byte: {byte}");
/// }
///
/// assert!(view.is_empty());
/// # }
/// # }
/// ```
///
/// [`BytesBuf`]: crate::BytesBuf
/// [`copied_from_slice()`]: Self::copied_from_slice
/// [`concat()`]: Self::concat
/// [`append()`]: Self::append
/// [`range()`]: Self::range
/// [`from_views()`]: Self::from_views
/// [`clone()`]: Self::clone
/// [obtain access to a memory provider]: crate#producing-byte-sequences
#[derive(Debug)]
pub struct BytesView {
    identity: BufferIdentity,

    /// The spans of the byte sequence, stored in reverse order for efficient consumption
    /// by popping items off the end of the collection.
    pub(crate) spans_reversed: SmallVec<[Span; MAX_INLINE_SPANS]>,

    /// We cache the length so we do not have to recalculate it every time it is queried.
    len: usize,
}

impl Clone for BytesView {
    fn clone(&self) -> Self {
        Self {
            identity: BufferIdentity::new(),
            spans_reversed: self.spans_reversed.clone(),
            len: self.len,
        }
    }
}

impl BytesView {
    /// Creates a view over a zero-sized byte sequence.
    ///
    /// Use a [`BytesBuf`] to create a view over some actual data.
    ///
    /// [`BytesBuf`]: crate::BytesBuf
    #[must_use]
    pub const fn new() -> Self {
        Self {
            identity: BufferIdentity::new(),
            spans_reversed: SmallVec::new_const(),
            len: 0,
        }
    }

    pub(crate) fn take_identity(&self) -> crate::telemetry::TransferredIdentity {
        self.identity.take()
    }

    pub(crate) fn replace_identity(&self, id: crate::telemetry::TransferredIdentity) {
        self.identity.replace(id);
    }

    pub(crate) fn from_spans_reversed(spans_reversed: SmallVec<[Span; MAX_INLINE_SPANS]>) -> Self {
        #[cfg(debug_assertions)]
        spans_reversed.iter().for_each(|span| assert!(!span.is_empty()));

        // We can use this to fine-tune the inline span count once we have real-world data.
        #[cfg(feature = "std")]
        VIEW_CREATED_SPANS.with(|x| x.observe(spans_reversed.len()));

        let len = spans_reversed.iter().fold(0_usize, |acc, span: &Span| {
            acc.checked_add(span.len() as usize)
                .expect("attempted to create a BytesView larger than usize::MAX bytes")
        });

        Self {
            identity: BufferIdentity::new(),
            spans_reversed,
            len,
        }
    }

    /// Concatenates a number of existing byte sequences, yielding a combined view.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let header = BytesView::copied_from_slice(b"HTTP/1.1 ", &memory);
    /// let status = BytesView::copied_from_slice(b"200 ", &memory);
    /// let message = BytesView::copied_from_slice(b"OK", &memory);
    ///
    /// let response_line = BytesView::from_views([header, status, message]);
    ///
    /// assert_eq!(response_line.len(), 15);
    /// assert_eq!(response_line, b"HTTP/1.1 200 OK");
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the resulting view would be larger than `usize::MAX` bytes.
    pub fn from_views<I>(views: I) -> Self
    where
        I: IntoIterator<Item = Self>,
        <I as IntoIterator>::IntoIter: iter::DoubleEndedIterator,
    {
        // The two-level flat_map exposes no usable size hint, so the SmallVec grows on the fly.
        // Counting the spans in a first pass to pre-size the SmallVec is deliberately avoided: it
        // trims SmallVec reallocations and lowers the simulated instruction count, but the extra
        // counting traversal measured as a net wall-clock regression, costing more than the
        // reallocations it saves. The spans are moved out of the input views rather than cloned, so
        // this path performs no BlockRef refcount traffic or block allocation.

        // For a given input ABC123.
        let spans_reversed: SmallVec<_> = views
            .into_iter()
            // We first reverse the views: 123ABC.
            .rev()
            // And from inside each view we take the reversed spans: 321CBA.
            .flat_map(|view| view.spans_reversed)
            // Which become our final SmallVec of spans. Great success!
            .collect();

        Self::from_spans_reversed(spans_reversed)
    }

    /// Creates a `BytesView` by copying the contents of a `&[u8]`.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # struct TcpConnection;
    /// # impl TcpConnection {
    /// #     fn memory(&self) -> impl bytesbuf::mem::Memory { bytesbuf::mem::GlobalPool::new() }
    /// # }
    /// # let tcp_connection = TcpConnection;
    /// use bytesbuf::BytesView;
    ///
    /// const CONTENT_TYPE_KEY: &[u8] = b"Content-Type: ";
    ///
    /// let header_key = BytesView::copied_from_slice(CONTENT_TYPE_KEY, &tcp_connection.memory());
    ///
    /// assert_eq!(header_key.len(), 14);
    /// # }
    /// # }
    /// ```
    ///
    /// # Reusing without copying
    ///
    /// There is intentionally no mechanism in the `bytesbuf` crate to reference an existing
    /// `&[u8]` without copying the contents, even if it has a `'static` lifetime.
    ///
    /// The purpose of this limitation is to discourage accidentally involving arbitrary
    /// memory in high-performance I/O workflows. For efficient I/O processing, data must
    /// be stored in memory configured according to the needs of the consuming I/O endpoint,
    /// which is not the case for an arbitrary `&'static [u8]`.
    ///
    /// To reuse memory allocations, you need to reuse `BytesView` instances themselves.
    /// See the `bb_reuse.rs` example in the `bytesbuf` crate for a detailed example.
    #[must_use]
    pub fn copied_from_slice(bytes: &[u8], memory: &impl Memory) -> Self {
        let mut buf = memory.reserve(bytes.len());
        buf.put_slice(bytes);
        buf.consume_all()
    }

    pub(crate) fn into_spans_reversed(self) -> SmallVec<[Span; MAX_INLINE_SPANS]> {
        self.spans_reversed
    }

    /// The number of bytes exposed through the view.
    ///
    /// Consuming bytes from the view reduces its length.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let mut view = BytesView::copied_from_slice(b"Hello", &memory);
    /// assert_eq!(view.len(), 5);
    ///
    /// _ = view.get_byte();
    /// assert_eq!(view.len(), 4);
    ///
    /// _ = view.get_u16_le();
    /// assert_eq!(view.len(), 2);
    /// # }
    /// # }
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        // Sanity check.
        debug_assert_eq!(self.len, self.spans_reversed.iter().map(|x| x.len() as usize).sum::<usize>());

        self.len
    }

    /// Whether the view is of a zero-sized byte sequence.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Extends the lifetime of the memory capacity backing this view.
    ///
    /// This can be useful when unsafe code is used to reference the contents of a `BytesView` and it
    /// is possible to reach a condition where the `BytesView` itself no longer exists, even though
    /// the contents are referenced (e.g. because the remaining references are in non-Rust code).
    pub fn extend_lifetime(&self) -> MemoryGuard {
        MemoryGuard::new(self.spans_reversed.iter().map(Span::block_ref).map(Clone::clone))
    }

    /// Returns a range of the byte sequence.
    ///
    /// The bounds logic only considers data currently present in the view.
    /// Any data already consumed is not considered part of the view.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let view = BytesView::copied_from_slice(b"Hello, world!", &memory);
    ///
    /// assert_eq!(view.range(0..5), b"Hello");
    /// assert_eq!(view.range(7..), b"world!");
    /// assert_eq!(view.range(..5), b"Hello");
    /// assert_eq!(view.range(..), b"Hello, world!");
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the provided range is outside the bounds of the view.
    #[must_use]
    pub fn range<R>(&self, range: R) -> Self
    where
        R: RangeBounds<usize>,
    {
        self.range_checked(range).expect("provided range out of view bounds")
    }

    /// Returns a range of the byte sequence or `None` if out of bounds.
    ///
    /// The bounds logic only considers data currently present in the view.
    /// Any data already consumed is not considered part of the view.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    #[expect(clippy::too_many_lines, reason = "acceptable for now")]
    pub fn range_checked<R>(&self, range: R) -> Option<Self>
    where
        R: RangeBounds<usize>,
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

        let required_len = bytes_until_range
            .checked_add(bytes_in_range)
            .expect("overflowing usize is impossible because we are calculating offset into usize-bounded range");

        if required_len > self.len() {
            // Did not have enough data to cover the range.
            return None;
        }

        if bytes_in_range == 0 {
            // Empty sequence is empty.
            return Some(Self::new());
        }

        // Fast path: when the entire requested range lies within the first span - always true for
        // a single-span view (the common case for a sequence backed by one memory block), and also
        // common when peeking a header at the front of a multi-span view - the result is just a
        // sub-slice of that one span, needing no cross-span accounting.
        if let Some(first) = self.spans_reversed.last()
            && required_len <= first.len() as usize
        {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "required_len <= first.len() <= BlockSize::MAX, so both casts are lossless"
            )]
            let (start, end) = (bytes_until_range as BlockSize, required_len as BlockSize);

            let mut spans_reversed = SmallVec::with_capacity(1);
            spans_reversed.push(first.slice(start..end));

            return Some(Self {
                identity: BufferIdentity::new(),
                spans_reversed,
                len: bytes_in_range,
            });
        }

        // Take the spans from the end of our spans_reversed (the logical beginning), while taking
        // bytes in each span from the beginning of the span. We implement this in two passes:
        // 1. Identify relevant range of spans. The idea is that our slice may just be a tiny
        //    subset of the entire sequence and we should not be processing parts of the sequence
        //    that do not matter (either because they are before the slice or after it).
        // 2. Within the relevant spans, skip to the relevant bytes, take them, and ignore the rest.
        //    This may range across any number of spans, though due to the pre-filtering in step 1
        //    we know that we only need to skip the head/tail in the first and last span.

        // Our accounting is all "logical", content-based.
        // These are the outputs from the first pass.
        let mut spans_until_range: usize = 0;
        let mut spans_in_range: usize = 0;
        let mut bytes_to_skip_in_first_relevant_span: BlockSize = 0;
        let mut bytes_to_leave_in_last_relevant_span: BlockSize = 0;

        {
            let mut pass1_bytes_until_range = bytes_until_range;
            let mut pass1_bytes_in_range = bytes_in_range;

            for span in self.spans_reversed.iter().rev() {
                let bytes_in_span = span.len();
                let bytes_in_span_usize = bytes_in_span as usize;

                if pass1_bytes_until_range > 0 && bytes_in_span_usize <= pass1_bytes_until_range {
                    // This entire span is uninteresting for us - skip.
                    spans_until_range = spans_until_range
                        .checked_add(1)
                        .expect("overflowing usize is impossible because we are calculating chunks within usize-bounded range");
                    pass1_bytes_until_range = pass1_bytes_until_range
                        .checked_sub(bytes_in_span_usize)
                        .expect("somehow ended up with negative bytes remaining until range start - only possible if the math is wrong");
                    continue;
                }

                // If we got to this point, it is an interesting span.

                // If this is the last span, we need to account for the bytes we are leaving behind.
                bytes_to_leave_in_last_relevant_span = bytes_in_span;

                // If we are at this point, pass1_bytes_until_range is either zero or points to some
                // position within this span, so it is now `BlockSize` bounded.
                let pass1_bytes_until_range_block_size = pass1_bytes_until_range.try_into().expect("we are supposedly indicating a position inside a span but the offset is larger than a memory block range - algorithm error");

                // We may still have some prefix to remove, so not every byte is relevant.
                if pass1_bytes_until_range != 0 {
                    bytes_to_skip_in_first_relevant_span = pass1_bytes_until_range_block_size;

                    // The first span might also be the last span.
                    bytes_to_leave_in_last_relevant_span = bytes_to_leave_in_last_relevant_span
                        .checked_sub(bytes_to_skip_in_first_relevant_span)
                        .expect("somehow ended up with negative bytes remaining in span - only possible if the math is wrong");
                }

                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the usize never contains a value outside bounds of BlockSize - guarded by min()"
                )]
                let relevant_bytes_in_span = ((bytes_in_span
                    .checked_sub(pass1_bytes_until_range_block_size)
                    .expect("somehow ended up with negative bytes remaining in span - only possible if the math is wrong")
                    as usize)
                    .min(pass1_bytes_in_range)) as BlockSize;

                bytes_to_leave_in_last_relevant_span = bytes_to_leave_in_last_relevant_span
                    .checked_sub(relevant_bytes_in_span)
                    .expect("somehow ended up with negative bytes remaining in span - only possible if the math is wrong");

                // Whatever happened, we have reached the relevant range now.
                spans_in_range = spans_in_range
                    .checked_add(1)
                    .expect("overflowing usize is impossible because we are calculating chunks within usize-bounded range");

                pass1_bytes_until_range = 0;

                pass1_bytes_in_range = pass1_bytes_in_range
                    .checked_sub(relevant_bytes_in_span as usize)
                    .expect("somehow ended up with negative bytes remaining in range - only possible if the math is wrong");

                if pass1_bytes_in_range == 0 {
                    // We have reached the end of the range - remaining spans are not interesting.
                    break;
                }
            }
        }

        let relevant_spans = self.spans_reversed.iter().rev().skip(spans_until_range).take(spans_in_range);

        let mut bytes_remaining_in_range = bytes_in_range;

        // We skip bytes_to_skip_in_first_relevant_span.
        // Then we take until bytes_remaining_in_range runs out.
        // The end. We know that every span is relevant now.

        // NB! We have to for-iterate over the relevant spans and not blindly use .map() because
        // .map() is lazy and may be evaluated in a completely different order from what we would
        // expect "logically". The easiest way for us to control iteration order is to for-loop.
        let mut slice_spans = SmallVec::with_capacity(spans_in_range);

        // These are in REVERSE ORDER, same as we use in storage. So we start with the last span.
        for span in relevant_spans.rev() {
            let mut bytes_to_even_consider = span.len();

            // If this is nonzero, we must be looking at the last relevant span.
            if bytes_to_leave_in_last_relevant_span > 0 {
                bytes_to_even_consider = bytes_to_even_consider
                    .checked_sub(bytes_to_leave_in_last_relevant_span)
                    .expect("somehow ended up with negative bytes remaining in span - only possible if the math is wrong");

                bytes_to_leave_in_last_relevant_span = 0;
            }

            #[expect(
                clippy::cast_possible_truncation,
                reason = "the usize never contains a value outside bounds of BlockSize - guarded by min()"
            )]
            let mut max_take_bytes = (bytes_to_even_consider as usize).min(bytes_remaining_in_range) as BlockSize;

            // Now if this is the first logical span (last in our iteration), we need to skip
            // some from the start. The key challenge here is - how do we know it is the first?
            // Simply put - it is the first if it can supply all the remaining bytes.
            let is_first_span = bytes_remaining_in_range <= max_take_bytes as usize;

            if is_first_span && bytes_to_skip_in_first_relevant_span > 0 {
                let remainder_in_span = bytes_to_even_consider
                    .checked_sub(bytes_to_skip_in_first_relevant_span)
                    .expect("somehow ended up with negative bytes remaining in span - only possible if the math is wrong");

                max_take_bytes = max_take_bytes.min(remainder_in_span);

                bytes_remaining_in_range = bytes_remaining_in_range
                    .checked_sub(max_take_bytes as usize)
                    .expect("somehow ended up with negative bytes remaining - only possible if the math is wrong");

                let start = bytes_to_skip_in_first_relevant_span;
                let end = bytes_to_skip_in_first_relevant_span
                    .checked_add(max_take_bytes)
                    .expect("overflowing usize is impossible because we are calculating slice within usize-bounded range");

                bytes_to_skip_in_first_relevant_span = 0;

                slice_spans.push(span.slice(start..end));
            } else {
                bytes_remaining_in_range = bytes_remaining_in_range
                    .checked_sub(max_take_bytes as usize)
                    .expect("somehow ended up with negative bytes remaining - only possible if the math is wrong");

                slice_spans.push(span.slice(0..max_take_bytes));
            }
        }

        Some(Self {
            identity: BufferIdentity::new(),
            spans_reversed: slice_spans,
            len: bytes_in_range,
        })
    }

    /// Executes a function `f` on each slice, consuming them all.
    ///
    /// The slices that make up the view are iterated in order,
    /// providing each to `f`. The view becomes empty after this.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// // Create a multi-slice view by concatenating independent views.
    /// # let part1 = BytesView::copied_from_slice(b"Hello", &memory);
    /// # let part2 = BytesView::copied_from_slice(b", ", &memory);
    /// # let part3 = BytesView::copied_from_slice(b"world!", &memory);
    /// let mut view = BytesView::from_views([part1, part2, part3]);
    ///
    /// view.consume_all_slices(|slice| {
    ///     println!("Slice of {} bytes: {:?}", slice.len(), slice);
    /// });
    ///
    /// assert!(view.is_empty());
    /// # }
    /// # }
    /// ```
    pub fn consume_all_slices<F>(&mut self, mut f: F)
    where
        F: FnMut(&[u8]),
    {
        // TODO: This fn could just be .into_iter() - we have no real
        // need for the "consume pattern" here. Iterators are more idiomatic.
        while !self.is_empty() {
            let slice = self.first_slice();
            f(slice);
            self.advance(slice.len());
        }
    }

    /// References the first slice of bytes in the byte sequence.
    ///
    /// Returns an empty slice if the view is over a zero-sized byte sequence.
    #[doc = include_str!("../docs/snippets/sequence_memory_layout.md")]
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let mut view = BytesView::copied_from_slice(b"0123456789ABCDEF", &memory);
    ///
    /// // Read the first 10 bytes without assuming the length of first_slice().
    /// let mut ten_bytes = Vec::with_capacity(10);
    ///
    /// while ten_bytes.len() < 10 {
    ///     let slice = view.first_slice();
    ///
    ///     let bytes_to_take = slice.len().min(10 - ten_bytes.len());
    ///
    ///     ten_bytes.extend_from_slice(&slice[..bytes_to_take]);
    ///     view.advance(bytes_to_take);
    /// }
    ///
    /// assert_eq!(ten_bytes, b"0123456789");
    /// # }
    /// # }
    /// ```
    #[must_use]
    pub fn first_slice(&self) -> &[u8] {
        self.spans_reversed.last().map_or::<&[u8], _>(&[], |span| span)
    }

    /// Iterates over all the slices that make up this view, together with their metadata.
    ///
    /// Each item is a tuple of `(data, meta)` where `data` is a byte slice and `meta` is the
    /// optional metadata of the memory block backing that slice.
    #[doc = include_str!("../docs/snippets/sequence_memory_layout.md")]
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// # struct PageAlignedMemory;
    /// use bytesbuf::BytesView;
    ///
    /// # let part1 = BytesView::copied_from_slice(b"Hello", &memory);
    /// # let part2 = BytesView::copied_from_slice(b"World", &memory);
    /// let view = BytesView::from_views([part1, part2]);
    ///
    /// for (data, meta) in view.slices() {
    ///     let is_page_aligned = meta.is_some_and(|m| m.is::<PageAlignedMemory>());
    ///     println!(
    ///         "Slice of {} bytes (page-aligned: {is_page_aligned})",
    ///         data.len()
    ///     );
    /// }
    /// # }
    /// # }
    /// ```
    ///
    /// See the stand-alone example `bb_optimal_path.rs` in the `bytesbuf` crate for
    /// a more detailed example of how to make use of the slice metadata.
    pub fn slices(&self) -> BytesViewSlices<'_> {
        BytesViewSlices::new(self)
    }

    /// Copies the byte sequence into a new [`Vec<u8>`].
    ///
    /// The view itself is left unchanged.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let view = BytesView::copied_from_slice(b"Hello, world!", &memory);
    ///
    /// let bytes = view.to_vec();
    ///
    /// assert_eq!(bytes, b"Hello, world!");
    /// # }
    /// # }
    /// ```
    #[must_use]
    pub fn to_vec(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.len());

        for (slice, _meta) in self.slices() {
            bytes.extend_from_slice(slice);
        }

        bytes
    }

    /// Inspects the metadata of the [`first_slice()`].
    ///
    /// `None` if there is no metadata associated with the first slice or
    /// if the view is over a zero-sized byte sequence.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// # struct PageAlignedMemory;
    /// use bytesbuf::BytesView;
    ///
    /// let view = BytesView::copied_from_slice(b"Hello", &memory);
    ///
    /// let is_page_aligned = view
    ///     .first_slice_meta()
    ///     .is_some_and(|meta| meta.is::<PageAlignedMemory>());
    ///
    /// println!("First slice is page-aligned: {is_page_aligned}");
    /// # }
    /// # }
    /// ```
    ///
    /// See the stand-alone example `bb_optimal_path.rs` in the `bytesbuf` crate for
    /// a more detailed example of how to make use of the slice metadata.
    ///
    /// [`first_slice()`]: Self::first_slice
    #[must_use]
    pub fn first_slice_meta(&self) -> Option<&dyn BlockMeta> {
        self.spans_reversed.last().and_then(|span| span.block_ref().meta())
    }

    /// Removes the first `count` bytes from the front of the view.
    ///
    /// The consumed bytes are dropped from the view, moving any remaining bytes to the front.
    ///
    /// If permitted by memory layout considerations and reference counts, the memory capacity
    /// backing the dropped bytes is released back to the memory provider.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let mut view = BytesView::copied_from_slice(b"0123456789ABCDEF", &memory);
    ///
    /// // Read the first 10 bytes without assuming the length of first_slice().
    /// let mut ten_bytes = Vec::with_capacity(10);
    ///
    /// while ten_bytes.len() < 10 {
    ///     let slice = view.first_slice();
    ///
    ///     let bytes_to_take = slice.len().min(10 - ten_bytes.len());
    ///
    ///     ten_bytes.extend_from_slice(&slice[..bytes_to_take]);
    ///     view.advance(bytes_to_take);
    /// }
    ///
    /// assert_eq!(ten_bytes, b"0123456789");
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `count` is greater than the number of bytes remaining.
    pub fn advance(&mut self, mut count: usize) {
        self.len = self.len.checked_sub(count).expect("attempted to advance past end of the view");

        while count > 0 {
            let front = self
                .spans_reversed
                .last_mut()
                .expect("logic error - ran out of spans before advancing over their contents");
            let span_len = front.len() as usize;

            if count < span_len {
                // SAFETY: We must guarantee we advance in-bounds. The if statement guarantees that.
                unsafe {
                    front.advance(count);
                }
                break;
            }

            self.spans_reversed.pop();
            // Will never overflow because we already handled the count < span_len case.
            count = count.wrapping_sub(span_len);
        }
    }

    /// Reduces the cached length after the caller has removed `amount` bytes from the front spans.
    ///
    /// This keeps the cached length consistent with `spans_reversed` for the fused consumption
    /// fast paths, which pop or advance the front span directly and then record the removal here.
    /// The caller must have already removed exactly `amount` bytes from the front of the view.
    #[inline]
    pub(crate) fn shrink_len(&mut self, amount: usize) {
        // Cannot underflow: the caller removed `amount` bytes that the view was already covering.
        self.len = self.len.wrapping_sub(amount);
    }

    /// Appends another view to the end of this one.
    ///
    /// This is a zero-copy operation, reusing the memory capacity of the other view.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let mut greeting = BytesView::copied_from_slice(b"Hello, ", &memory);
    /// let name = BytesView::copied_from_slice(b"world!", &memory);
    ///
    /// greeting.append(name);
    ///
    /// assert_eq!(greeting, b"Hello, world!");
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the resulting view would be larger than `usize::MAX` bytes.
    pub fn append(&mut self, other: Self) {
        let new_len = self
            .len
            .checked_add(other.len)
            .expect("attempted to create a BytesView larger than usize::MAX bytes");
        if other.is_empty() {
            return;
        }
        if self.is_empty() {
            self.identity.replace(other.take_identity());
        } else {
            self.identity.clear();
        }
        self.len = new_len;

        self.spans_reversed.insert_many(0, other.spans_reversed);
    }

    /// Returns a new view that concatenates this view with another.
    ///
    /// This is a zero-copy operation, reusing the memory capacity of the other view.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::BytesView;
    ///
    /// let greeting = BytesView::copied_from_slice(b"Hello, ", &memory);
    /// let name = BytesView::copied_from_slice(b"world!", &memory);
    ///
    /// let message = greeting.concat(name);
    ///
    /// // Original view is unchanged.
    /// assert_eq!(greeting, b"Hello, ");
    /// // New view contains the concatenation.
    /// assert_eq!(message, b"Hello, world!");
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the resulting view would be larger than `usize::MAX` bytes.
    #[must_use]
    pub fn concat(&self, other: Self) -> Self {
        let mut new_view = self.clone();
        new_view.append(other);
        new_view
    }
}

#[cfg(feature = "seismograph")]
impl seismograph_io::Buffer for BytesView {
    fn recording_state(&self) -> seismograph_io::BufferState {
        seismograph_io::BufferState::new(Some(self.identity.get_or_allocate()), self.len(), self.spans_reversed.len())
    }
}

impl Default for BytesView {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for BytesView {
    fn eq(&self, other: &Self) -> bool {
        // We do not care about the structure, only the contents.
        if self.len() != other.len() {
            return false;
        }

        // The two views may have differently sized spans, so we only compare in steps
        // of the smallest span size offered by either view.

        // We iterate over spans_reversed using indices to avoid cloning the views,
        // which would increment atomic reference counts for every span.
        let mut self_span_idx = self.spans_reversed.len();
        let mut self_offset: usize = 0;
        let mut other_span_idx = other.spans_reversed.len();
        let mut other_offset: usize = 0;
        let mut remaining_bytes = self.len();

        while remaining_bytes > 0 {
            let self_span: &[u8] = &self.spans_reversed[self_span_idx - 1];
            let other_span: &[u8] = &other.spans_reversed[other_span_idx - 1];

            let self_slice = self_span
                .get(self_offset..)
                .expect("offset only advances within span length, reset to 0 on span boundary");
            let other_slice = other_span
                .get(other_offset..)
                .expect("offset only advances within span length, reset to 0 on span boundary");

            let comparison_len = NonZero::new(self_slice.len().min(other_slice.len()))
                .expect("both views said there are remaining bytes but we got an empty slice from at least one of them");

            let self_slice = self_slice.get(..comparison_len.get()).expect("guarded by min() above");
            let other_slice = other_slice.get(..comparison_len.get()).expect("guarded by min() above");

            if self_slice != other_slice {
                // Something is different. That is enough for a determination.
                return false;
            }

            // Advance within or past the current spans.
            self_offset += comparison_len.get();
            debug_assert!(self_offset <= self_span.len(), "guarded by min() above to never exceed span length");
            if self_offset == self_span.len() {
                self_span_idx -= 1;
                self_offset = 0;
            }

            other_offset += comparison_len.get();
            debug_assert!(
                other_offset <= other_span.len(),
                "guarded by min() above to never exceed span length"
            );
            if other_offset == other_span.len() {
                other_span_idx -= 1;
                other_offset = 0;
            }

            remaining_bytes = remaining_bytes
                .checked_sub(comparison_len.get())
                .expect("impossible to consume more bytes from the sequences than are remaining");
        }

        debug_assert_eq!(remaining_bytes, 0);

        true
    }
}

impl PartialEq<&[u8]> for BytesView {
    fn eq(&self, other: &&[u8]) -> bool {
        let mut other = *other;

        // We do not care about the structure, only the contents.

        if self.len() != other.len() {
            return false;
        }

        // We iterate over spans_reversed using an index to avoid cloning the view,
        // which would increment atomic reference counts for every span.
        let mut span_idx = self.spans_reversed.len();
        let mut remaining_bytes = self.len();

        while remaining_bytes > 0 {
            let span: &[u8] = &self.spans_reversed[span_idx - 1];
            let self_slice = span;

            let slice_size = NonZero::new(self_slice.len())
                .expect("both sides of the comparison said there are remaining bytes but we got an empty slice from at least one of them");

            let self_slice = self_slice.get(..slice_size.get()).expect("already checked that remaining > 0");
            let other_slice = other.get(..slice_size.get()).expect("already checked that remaining > 0");

            if self_slice != other_slice {
                return false;
            }

            span_idx -= 1;

            other = other.get(slice_size.get()..).expect("guarded by length check above");

            remaining_bytes = remaining_bytes
                .checked_sub(slice_size.get())
                .expect("impossible to consume more bytes from the sequences than are remaining");
        }

        debug_assert_eq!(remaining_bytes, 0);

        true
    }
}

impl PartialEq<BytesView> for &[u8] {
    fn eq(&self, other: &BytesView) -> bool {
        other.eq(self)
    }
}

impl<const LEN: usize> PartialEq<&[u8; LEN]> for BytesView {
    fn eq(&self, other: &&[u8; LEN]) -> bool {
        self.eq(&other.as_slice())
    }
}

impl<const LEN: usize> PartialEq<BytesView> for &[u8; LEN] {
    fn eq(&self, other: &BytesView) -> bool {
        other.eq(&self.as_slice())
    }
}

impl Eq for BytesView {}

/// Block size used when hashing a [`BytesView`], chosen as one cache line so that small views
/// (the common case) are hashed with a single `write` call while larger views still drive the
/// hasher with a bounded number of bulk writes. The exact value only affects performance; any
/// fixed constant yields a correct, segmentation-independent hash.
const HASH_BLOCK_BYTES: usize = 64;

impl Hash for BytesView {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Equality is defined by the logical byte sequence, independent of how it is segmented
        // into spans, so hashing must be independent of segmentation too. The `Hasher` contract
        // does not guarantee that `write(a); write(b)` produces the same result as
        // `write(a ++ b)`, so hashing one span at a time would let span boundaries leak into the
        // hash: two equal views with different segmentation could then produce different hashes,
        // violating the `Hash`/`Eq` contract and breaking `HashMap` lookups.
        //
        // Instead we re-chunk the logical byte sequence into fixed-size blocks using a small stack
        // buffer and feed those blocks to the hasher. Block boundaries depend only on the byte
        // content (a full block every `HASH_BLOCK_BYTES` bytes, then a final partial block), never
        // on span boundaries, so equal views always drive the hasher with an identical sequence of
        // `write` calls. This is allocation-free, unlike gathering the bytes into a contiguous
        // buffer.
        //
        // We also write the length as a prefix to keep the hash prefix-free, matching how the
        // standard library hashes byte slices.
        self.len.hash(state);

        let mut block = [0_u8; HASH_BLOCK_BYTES];
        let mut filled = 0_usize;

        // We iterate over spans_reversed in reverse (logical order) using references, which does
        // not clone the spans and therefore does not touch their atomic reference counts.
        for span in self.spans_reversed.iter().rev() {
            let mut remaining: &[u8] = span;
            while !remaining.is_empty() {
                // Fast path: when the buffer is empty we are aligned to a block boundary, so if the
                // span holds at least a full block we can hash it directly from the span without
                // copying it through the stack buffer. The resulting `write` call carries exactly
                // the same bytes as the buffered path would, so the canonical call sequence is
                // preserved while avoiding a copy of large contiguous spans.
                if filled == 0 && remaining.len() >= HASH_BLOCK_BYTES {
                    let (full_block, rest) = remaining.split_at(HASH_BLOCK_BYTES);
                    state.write(full_block);
                    remaining = rest;
                    continue;
                }

                let take = (HASH_BLOCK_BYTES - filled).min(remaining.len());
                block[filled..filled + take].copy_from_slice(&remaining[..take]);
                filled += take;
                remaining = &remaining[take..];

                if filled == HASH_BLOCK_BYTES {
                    state.write(&block);
                    filled = 0;
                }
            }
        }

        if filled > 0 {
            state.write(&block[..filled]);
        }
    }
}

/// Iterator over the slices of a [`BytesView`] and their metadata.
///
/// Returned by [`BytesView::slices()`] and provides each slice together with its
/// associated memory block metadata.
#[must_use]
#[derive(Debug)]
pub struct BytesViewSlices<'s> {
    view: &'s BytesView,

    // Index of the most recent that we have already returned.
    //
    // Index into spans_reversed, counting from the end (the logical first span).
    // Starts at spans_reversed.len() (one past the end) and counts down toward 0.
    previous_span_index: usize,
}

impl<'s> BytesViewSlices<'s> {
    pub(crate) fn new(view: &'s BytesView) -> Self {
        Self {
            view,
            previous_span_index: view.spans_reversed.len(),
        }
    }
}

impl<'s> Iterator for BytesViewSlices<'s> {
    type Item = (&'s [u8], Option<&'s dyn BlockMeta>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.previous_span_index == 0 {
            return None;
        }

        self.previous_span_index -= 1;
        let span = &self.view.spans_reversed[self.previous_span_index];
        let slice: &'s [u8] = span;
        let meta: Option<&'s dyn BlockMeta> = span.block_ref().meta();

        Some((slice, meta))
    }
}

#[cfg(feature = "std")]
const SPAN_COUNT_BUCKETS: &[Magnitude] = &[0, 1, 2, 4, 8, 16, 32];

#[cfg(feature = "std")]
std::thread_local! {
    static VIEW_CREATED_SPANS: Event = Event::builder()
        .name("bytesbuf_view_created_spans")
        .histogram(SPAN_COUNT_BUCKETS)
        .build();
}

#[cfg(test)]
mod tests {
    // The size assertion includes the private BufferIdentity layout.
    use super::*;

    #[test]
    fn size_change_detector() {
        // The point of this is not to say that we expect it to have a specific size but to allow
        // us to easily detect when the size changes and (if we choose to) bless the change.
        // We assume 64-bit pointers - any support for 32-bit is problem for the future.
        assert_eq!(size_of::<BytesView>(), 272 + size_of::<BufferIdentity>());
    }
}
