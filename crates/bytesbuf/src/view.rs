// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::io::IoSlice;
use std::marker::PhantomData;
use std::num::NonZero;
use std::ops::{Bound, RangeBounds};
use std::{iter, mem};

use nm::{Event, Magnitude};
use smallvec::SmallVec;

use crate::{BlockSize, MAX_INLINE_SPANS, Memory, MemoryGuard, Span};

/// A view over a sequence of immutable bytes.
///
/// Note that only the contents are immutable - the view itself can be mutated in terms of progressively
/// marking the byte sequence as consumed until the view becomes empty.
///
/// To create a `BytesView`, use a [`BytesBuf`][3] or clone/slice an existing `BytesView`.
#[doc = include_str!("../doc/snippets/sequence_memory_layout.md")]
///
/// [3]: crate::BytesBuf
#[derive(Clone, Debug)]
pub struct BytesView {
    /// The spans of the byte sequence, stored in reverse order for efficient consumption
    /// by popping items off the end of the collection.
    pub(crate) spans_reversed: SmallVec<[Span; MAX_INLINE_SPANS]>,

    /// We cache the length so we do not have to recalculate it every time it is queried.
    len: usize,
}

impl BytesView {
    /// Returns a view over a zero-sized byte sequence.
    ///
    /// Use a [`BytesBuf`][1] to create a view over some actual data.
    ///
    /// [1]: crate::BytesBuf
    #[cfg_attr(test, mutants::skip)] // Generates no-op mutations, not useful.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            spans_reversed: SmallVec::new_const(),
            len: 0,
        }
    }

    pub(crate) fn from_spans_reversed(spans_reversed: SmallVec<[Span; MAX_INLINE_SPANS]>) -> Self {
        #[cfg(debug_assertions)]
        spans_reversed.iter().for_each(|span| assert!(!span.is_empty()));

        // We can use this to fine-tune the inline span count once we have real-world data.
        VIEW_CREATED_SPANS.with(|x| x.observe(spans_reversed.len()));

        let len = spans_reversed.iter().map(|x| x.len() as usize).sum();

        Self { spans_reversed, len }
    }

    /// Concatenates a number of spans, yielding a view that combines the spans.
    ///
    /// Later changes made to the input spans will not be reflected in the resulting view.
    #[cfg(test)]
    pub(crate) fn from_spans<I>(spans: I) -> Self
    where
        I: IntoIterator<Item = Span>,
        <I as IntoIterator>::IntoIter: iter::DoubleEndedIterator,
    {
        let spans_reversed = spans.into_iter().rev().collect::<SmallVec<_>>();

        Self::from_spans_reversed(spans_reversed)
    }

    /// Concatenates a number of existing byte sequences, yielding a combined view.
    ///
    /// Later changes made to the input views will not be reflected in the resulting view.
    pub fn from_views<I>(views: I) -> Self
    where
        I: IntoIterator<Item = Self>,
        <I as IntoIterator>::IntoIter: iter::DoubleEndedIterator,
    {
        // Note that this requires the SmallVec to resize on the fly because thanks to the
        // two-level mapping here, there is no usable size hint that lets it know the size in
        // advance. If we had the span count here, we could avoid some allocations.

        // For a given input ABC123.
        let spans_reversed: SmallVec<_> = views
            .into_iter()
            // We first reverse the views: 123ABC.
            .rev()
            // And from inside each view we take the reversed spans: 321CBA.
            .flat_map(|view| view.spans_reversed)
            // Which become our final SmallVec of spans. Great success!
            .collect();

        // We can use this to fine-tune the inline span count once we have real-world data.
        VIEW_CREATED_SPANS.with(|x| x.observe(spans_reversed.len()));

        let len = spans_reversed.iter().map(|x: &Span| x.len() as usize).sum();

        Self { spans_reversed, len }
    }

    /// Creates a `BytesView` by copying the contents of a `&[u8]`.
    ///
    /// There is intentionally no mechanism in `bytesbuf` to reference an existing `&[u8]`
    /// without copying, even if `'static`, because high-performance I/O requires all data
    /// to exist in memory owned by the I/O subsystem. Reusing arbitrary byte slices is
    /// not supported in order to discourage design practices that would work against this
    /// goal. To reuse memory allocations, reuse the `BytesView` itself.
    #[must_use]
    pub fn copied_from_slice(bytes: &[u8], memory_provider: &impl Memory) -> Self {
        let mut buffer = memory_provider.reserve(bytes.len());
        buffer.put_slice(bytes);
        buffer.consume_all()
    }

    pub(crate) fn into_spans_reversed(self) -> SmallVec<[Span; MAX_INLINE_SPANS]> {
        self.spans_reversed
    }

    /// The number of bytes exposed through the view.
    ///
    /// Consuming bytes from the view reduces its length.
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

    /// Returns a sub-view over a range of the byte sequence.
    ///
    /// The bounds logic only considers data currently present in the view.
    /// Any data already consumed is not considered part of the view.
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

    /// Returns a sub-view over a range of the byte sequence or `None` if out of bounds.
    ///
    /// The bounds logic only considers data currently present in the view.
    /// Any data already consumed is not considered part of the view.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    #[expect(clippy::too_many_lines, reason = "acceptable for now")]
    #[cfg_attr(test, mutants::skip)] // Mutations include impossible conditions that we cannot test as well as mutations that are functionally equivalent.
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
            spans_reversed: slice_spans,
            len: bytes_in_range,
        })
    }

    /// Executes a function `f` on each slice, consuming them all.
    ///
    /// The slices that make up the view are iterated in order,
    /// providing each to `f`. The view becomes empty after this.
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
    /// There are no guarantees on the length of each slice. In a view over a non-empty
    /// byte sequence, each slice may contain anywhere between 1 byte and all bytes of
    /// the sequence.
    ///
    /// Returns an empty slice if the view is over a zero-sized byte sequence.
    #[cfg_attr(test, mutants::skip)] // Mutating this can cause infinite loops.
    #[must_use]
    pub fn first_slice(&self) -> &[u8] {
        self.spans_reversed.last().map_or::<&[u8], _>(&[], |span| span)
    }

    /// Fills an array with `IoSlice`s representing this view.
    ///
    /// Returns the number of elements written into `dst`. If there is not enough space in `dst`
    /// to represent the entire view, only as many slices as fit will be written.
    ///
    /// See also [`slices()`] for a version that fills an array of regular slices instead of `IoSlice`s.
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn io_slices<'a>(&'a self, dst: &mut [IoSlice<'a>]) -> usize {
        if dst.is_empty() {
            return 0;
        }

        // How many slices can we fill?
        let slice_count = self.spans_reversed.len().min(dst.len());

        // Note that IoSlice has a length limit of u32::MAX. Our spans are also limited to u32::MAX
        // by memory manager internal limits (MAX_BLOCK_SIZE), so this is safe.
        for (i, span) in self.spans_reversed.iter().rev().take(slice_count).enumerate() {
            *dst.get_mut(i).expect("guarded by min()") = IoSlice::new(span);
        }

        slice_count
    }

    /// Fills an array with byte slices representing this view.
    ///
    /// Returns the number of elements written into `dst`. If there is not enough space in `dst`
    /// to represent the entire view, only as many slices as fit will be written.
    ///
    /// See also [`io_slices()`] for a version that fills an array of `IoSlice`s instead of regular byte slices.
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn slices<'a>(&'a self, dst: &mut [&'a [u8]]) -> usize {
        if dst.is_empty() {
            return 0;
        }

        // How many slices can we fill?
        let slice_count = self.spans_reversed.len().min(dst.len());

        for (i, span) in self.spans_reversed.iter().rev().take(slice_count).enumerate() {
            *dst.get_mut(i).expect("guarded by min()") = span;
        }

        slice_count
    }

    /// Inspects the metadata of the `first_slice()`.
    ///
    /// `None` if there is no metadata associated with the first slice or
    /// if the view is over a zero-sized byte sequence.
    #[must_use]
    pub fn first_slice_meta(&self) -> Option<&dyn Any> {
        self.spans_reversed.last().and_then(|span| span.block_ref().meta())
    }

    /// Iterates over the metadata of all the slices that make up the view.
    ///
    /// Each slice iterated over is a slice that would be returned by `first_slice()`
    /// at some point during the complete consumption of the data covered by a [`BytesView`].
    ///
    /// You may wish to iterate over the metadata to determine in advance which implementation
    /// strategy to use for a function, depending on what the metadata indicates about the
    /// configuration of the memory blocks backing the byte sequence.
    pub fn iter_slice_metas(&self) -> BytesViewSliceMetasIterator<'_> {
        BytesViewSliceMetasIterator::new(self)
    }

    /// Marks the first `count` bytes as consumed.
    ///
    /// The consumed bytes are dropped from the view, moving any remaining bytes to the front.
    ///
    /// # Panics
    ///
    /// Panics if `count` is greater than the number of bytes remaining.
    #[cfg_attr(test, mutants::skip)] // Mutating this can cause infinite loops.
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

    /// Appends another view to the end of this one.
    ///
    /// This is a zero-copy operation, reusing the memory capacity of the other view.
    ///
    /// # Panics
    ///
    /// Panics if the resulting view would be larger than `usize::MAX` bytes.
    pub fn append(&mut self, other: Self) {
        self.len = self
            .len
            .checked_add(other.len)
            .expect("attempted to create a BytesView larger than usize::MAX bytes");

        self.spans_reversed.insert_many(0, other.spans_reversed);
    }

    /// Returns a new view that concatenates this view with another.
    ///
    /// This is a zero-copy operation, reusing the memory capacity of the other view.
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

        let mut remaining_bytes = self.len();

        // The two views may have differently sized spans, so we only compare in steps
        // of the smallest span size offered by either view.

        // We clone the views to create windows that we slide over the contents.
        let mut self_view = self.clone();
        let mut other_view = other.clone();

        while remaining_bytes > 0 {
            let self_slice = self_view.first_slice();
            let other_slice = other_view.first_slice();

            let comparison_len = NonZero::new(self_slice.len().min(other_slice.len()))
                .expect("both views said there are remaining bytes but we got an empty slice from at least one of them");

            let self_slice = self_slice.get(..comparison_len.get()).expect("already checked that remaining > 0");
            let other_slice = other_slice.get(..comparison_len.get()).expect("already checked that remaining > 0");

            if self_slice != other_slice {
                // Something is different. That is enough for a determination.
                return false;
            }

            // Advance both views by the same amount.
            self_view.advance(comparison_len.get());
            other_view.advance(comparison_len.get());

            remaining_bytes = remaining_bytes
                .checked_sub(comparison_len.get())
                .expect("impossible to consume more bytes from the sequences than are remaining");
        }

        debug_assert_eq!(remaining_bytes, 0);
        debug_assert_eq!(self_view.len(), 0);
        debug_assert_eq!(other_view.len(), 0);

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

        let mut remaining_bytes = self.len();

        // We clone the sequence to create a temporary view that we slide over the contents.
        let mut self_view = self.clone();

        while remaining_bytes > 0 {
            let self_slice = self_view.first_slice();
            let slice_size = NonZero::new(self_slice.len())
                .expect("both sides of the comparison said there are remaining bytes but we got an empty slice from at least one of them");

            let self_slice = self_slice.get(..slice_size.get()).expect("already checked that remaining > 0");
            let other_slice = other.get(..slice_size.get()).expect("already checked that remaining > 0");

            if self_slice != other_slice {
                // Something is different. That is enough for a determination.
                return false;
            }

            // Advance the sequence by the same amount.
            self_view.advance(slice_size.get());
            other = other.get(slice_size.get()..).expect("guarded by min() above");

            remaining_bytes = remaining_bytes
                .checked_sub(slice_size.get())
                .expect("impossible to consume more bytes from the sequences than are remaining");
        }

        debug_assert_eq!(remaining_bytes, 0);
        debug_assert_eq!(self_view.len(), 0);
        debug_assert_eq!(other.len(), 0);

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

/// Iterator over all `first_slice_meta()` values of a [`BytesView`].
///
/// Returned by [`BytesView::iter_slice_metas()`][BytesView::iter_slice_metas] and allows you to
/// inspect the metadata of each slice that makes up the view without consuming any part of the view.
#[must_use]
#[derive(Debug)]
pub struct BytesViewSliceMetasIterator<'s> {
    // This starts off as a clone of the view, just for ease of implementation.
    // We consume the parts of the view we have already iterated over.
    view: BytesView,

    // We keep a reference to the view we are iterating over, even though
    // the current implementation does not use it (because a future one might).
    _parent: PhantomData<&'s BytesView>,
}

impl<'s> BytesViewSliceMetasIterator<'s> {
    pub(crate) fn new(view: &'s BytesView) -> Self {
        Self {
            view: view.clone(),
            _parent: PhantomData,
        }
    }
}

impl<'s> Iterator for BytesViewSliceMetasIterator<'s> {
    type Item = Option<&'s dyn Any>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.view.is_empty() {
            return None;
        }

        let meta = self.view.first_slice_meta();

        // SAFETY: It is normally not possible to return a self-reference from an iterator because
        // next() only has an implicit lifetime for `&self`, which cannot be named in `Item`.
        // However, we can take advantage of the fact that a `BlockRef` implementation is required
        // to guarantee that the metadata lives as long as any clone of the memory block. Because
        // the iterator has borrowed the parent `BytesView` we know that the memory block must live
        // for as long as the iterator lives.
        //
        // Therefor we can just re-stamp the return value with the 's lifetime to indicate that it
        // is valid for as long as the iterator has borrowed the parent BytesView for.
        let meta_with_s = unsafe { mem::transmute::<Option<&dyn Any>, Option<&'s dyn Any>>(meta) };

        // Seek forward to the next chunk before we return.
        self.view.advance(self.view.first_slice().len());

        Some(meta_with_s)
    }
}

const SPAN_COUNT_BUCKETS: &[Magnitude] = &[0, 1, 2, 4, 8, 16, 32];

thread_local! {
    static VIEW_CREATED_SPANS: Event = Event::builder()
        .name("bytesbuf_view_created_spans")
        .histogram(SPAN_COUNT_BUCKETS)
        .build();
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        clippy::needless_range_loop,
        clippy::arithmetic_side_effects,
        reason = "This is all fine in test code"
    )]

    use std::pin::pin;
    use std::thread;

    use new_zealand::nz;
    use static_assertions::assert_impl_all;
    use testing_aids::assert_panic;

    use super::*;
    use crate::testing::TestMemoryBlock;
    use crate::{BytesBuf, TransparentTestMemory, std_alloc_block};

    assert_impl_all!(BytesView: Send, Sync);

    #[test]
    fn smoke_test() {
        let mut span_builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        span_builder.put_slice(&1234_u64.to_ne_bytes());
        span_builder.put_slice(&16_u16.to_ne_bytes());

        let span1 = span_builder.consume(nz!(4));
        let span2 = span_builder.consume(nz!(3));
        let span3 = span_builder.consume(nz!(3));

        assert_eq!(0, span_builder.remaining_capacity());
        assert_eq!(span1.len(), 4);
        assert_eq!(span2.len(), 3);
        assert_eq!(span3.len(), 3);

        let mut view = BytesView::from_spans(vec![span1, span2, span3]);

        assert!(!view.is_empty());
        assert_eq!(10, view.len());

        let slice = view.first_slice();
        assert_eq!(4, slice.len());

        // We read 8 bytes here, so should land straight inside span3.
        assert_eq!(view.get_num_ne::<u64>(), 1234);

        assert_eq!(2, view.len());

        let slice = view.first_slice();
        assert_eq!(2, slice.len());

        assert_eq!(view.get_num_ne::<u16>(), 16);

        assert_eq!(0, view.len());
        assert!(view.is_empty());
    }

    #[test]
    fn oob_is_panic() {
        let mut span_builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        span_builder.put_slice(&1234_u64.to_ne_bytes());
        span_builder.put_slice(&16_u16.to_ne_bytes());

        let span1 = span_builder.consume(nz!(4));
        let span2 = span_builder.consume(nz!(3));
        let span3 = span_builder.consume(nz!(3));

        let mut view = BytesView::from_spans(vec![span1, span2, span3]);

        assert_eq!(10, view.len());

        assert_eq!(view.get_num_ne::<u64>(), 1234);
        assert_panic!(_ = view.get_num_ne::<u32>()); // Reads 4 but only has 2 remaining.
    }

    #[test]
    fn extend_lifetime_references_all_blocks() {
        // We need to detect here whether a block is being released (i.e. ref count goes to zero).

        // SAFETY: We are not allowed to drop this until all BlockRef are gone. This is fine
        // because it is dropped at the end of the function, after all BlockRef instances.
        let block1 = unsafe { TestMemoryBlock::new(nz!(8), None) };
        let block1 = pin!(block1);

        // SAFETY: We are not allowed to drop this until all BlockRef are gone. This is fine
        // because it is dropped at the end of the function, after all BlockRef instances.
        let block2 = unsafe { TestMemoryBlock::new(nz!(8), None) };
        let block2 = pin!(block2);

        let guard = {
            // SAFETY: We guarantee exclusive access to the memory capacity.
            let mut span_builder1 = unsafe { block1.as_ref().to_block() }.into_span_builder();
            // SAFETY: We guarantee exclusive access to the memory capacity.
            let mut span_builder2 = unsafe { block2.as_ref().to_block() }.into_span_builder();

            span_builder1.put_slice(&1234_u64.to_ne_bytes());
            span_builder2.put_slice(&1234_u64.to_ne_bytes());

            let span1 = span_builder1.consume(nz!(8));
            let span2 = span_builder2.consume(nz!(8));

            let view = BytesView::from_spans(vec![span1, span2]);

            view.extend_lifetime()
        };

        // The sequence was destroyed and all BlockRefs it was holding are gone.
        // However, the lifetime guard is still alive and has a BlockRef.

        assert_eq!(block1.ref_count(), 1);
        assert_eq!(block2.ref_count(), 1);

        drop(guard);

        // And now they should all be dead.
        assert_eq!(block1.ref_count(), 0);
        assert_eq!(block2.ref_count(), 0);
    }

    #[test]
    fn from_views() {
        let mut span_builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        span_builder.put_slice(&1234_u64.to_ne_bytes());
        span_builder.put_slice(&5678_u64.to_ne_bytes());

        let span1 = span_builder.consume(nz!(8));
        let span2 = span_builder.consume(nz!(8));

        let view1 = BytesView::from_spans(vec![span1]);
        let view2 = BytesView::from_spans(vec![span2]);

        let mut combined_view = BytesView::from_views(vec![view1, view2]);

        assert_eq!(16, combined_view.len());

        assert_eq!(combined_view.get_num_ne::<u64>(), 1234);
        assert_eq!(combined_view.get_num_ne::<u64>(), 5678);
    }

    #[test]
    fn empty_view() {
        let view = BytesView::default();

        assert!(view.is_empty());
        assert_eq!(0, view.len());
        assert_eq!(0, view.first_slice().len());
    }

    #[test]
    fn slice_from_single_span_view() {
        // A very simple view to start with, consisting of just one 100 byte span.
        let span_builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        let mut buf = BytesBuf::from_span_builders([span_builder]);

        for i in 0..100 {
            buf.put_byte(i);
        }

        let view = buf.consume_all();

        let mut sliced_view = view.range(50..55);

        assert_eq!(5, sliced_view.len());
        assert_eq!(100, view.len());

        assert_eq!(50, sliced_view.get_byte());

        assert_eq!(4, sliced_view.len());
        assert_eq!(100, view.len());

        assert_eq!(51, sliced_view.get_byte());
        assert_eq!(52, sliced_view.get_byte());
        assert_eq!(53, sliced_view.get_byte());
        assert_eq!(54, sliced_view.get_byte());

        assert_eq!(0, sliced_view.len());

        assert!(view.range_checked(0..101).is_none());
        assert!(view.range_checked(100..101).is_none());
        assert!(view.range_checked(101..101).is_none());
    }

    #[test]
    fn slice_from_multi_span_view() {
        const SPAN_SIZE: NonZero<BlockSize> = nz!(10);

        // A multi-span view, 10 bytes x10.
        let span_builders = iter::repeat_with(|| std_alloc_block::allocate(SPAN_SIZE).into_span_builder())
            .take(10)
            .collect::<Vec<_>>();

        let mut buf = BytesBuf::from_span_builders(span_builders);

        for i in 0..100 {
            buf.put_byte(i);
        }

        let view = buf.consume_all();

        let mut first5 = view.range(0..5);
        assert_eq!(5, first5.len());
        assert_eq!(100, view.len());
        assert_eq!(0, first5.get_byte());

        let mut last5 = view.range(95..100);
        assert_eq!(5, last5.len());
        assert_eq!(100, view.len());
        assert_eq!(95, last5.get_byte());

        let mut middle5 = view.range(49..54);
        assert_eq!(5, middle5.len());
        assert_eq!(100, view.len());
        assert_eq!(49, middle5.get_byte());
        assert_eq!(50, middle5.get_byte());
        assert_eq!(51, middle5.get_byte());
        assert_eq!(52, middle5.get_byte());
        assert_eq!(53, middle5.get_byte());

        assert!(view.range_checked(0..101).is_none());
        assert!(view.range_checked(100..101).is_none());
        assert!(view.range_checked(101..101).is_none());
    }

    #[test]
    fn slice_indexing_kinds() {
        let span_builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        let mut buf = BytesBuf::from_span_builders([span_builder]);
        buf.put_byte(0);
        buf.put_byte(1);
        buf.put_byte(2);
        buf.put_byte(3);
        buf.put_byte(4);
        buf.put_byte(5);

        let sequence = buf.consume_all();

        let mut middle_four = sequence.range(1..5);
        assert_eq!(4, middle_four.len());
        assert_eq!(1, middle_four.get_byte());
        assert_eq!(2, middle_four.get_byte());
        assert_eq!(3, middle_four.get_byte());
        assert_eq!(4, middle_four.get_byte());

        let mut middle_four = sequence.range(1..=4);
        assert_eq!(4, middle_four.len());
        assert_eq!(1, middle_four.get_byte());
        assert_eq!(2, middle_four.get_byte());
        assert_eq!(3, middle_four.get_byte());
        assert_eq!(4, middle_four.get_byte());

        let mut last_two = sequence.range(4..);
        assert_eq!(2, last_two.len());
        assert_eq!(4, last_two.get_byte());
        assert_eq!(5, last_two.get_byte());

        let mut first_two = sequence.range(..2);
        assert_eq!(2, first_two.len());
        assert_eq!(0, first_two.get_byte());
        assert_eq!(1, first_two.get_byte());

        let mut first_two = sequence.range(..=1);
        assert_eq!(2, first_two.len());
        assert_eq!(0, first_two.get_byte());
        assert_eq!(1, first_two.get_byte());
    }

    #[test]
    fn slice_checked_with_excluded_start_bound() {
        let span_builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        let mut buf = BytesBuf::from_span_builders([span_builder]);
        buf.put_byte(0);
        buf.put_byte(1);
        buf.put_byte(2);
        buf.put_byte(3);
        buf.put_byte(4);
        buf.put_byte(5);
        buf.put_byte(6);
        buf.put_byte(7);
        buf.put_byte(8);

        let view = buf.consume_all();

        // Test with excluded start bound: (Bound::Excluded(1), Bound::Excluded(5))
        // This should be equivalent to 2..5 (items at indices 2, 3, 4)
        let sliced = view.range_checked((Bound::Excluded(1), Bound::Excluded(5)));
        assert!(sliced.is_some());
        let mut sliced = sliced.unwrap();
        assert_eq!(3, sliced.len());
        assert_eq!(2, sliced.get_byte());
        assert_eq!(3, sliced.get_byte());
        assert_eq!(4, sliced.get_byte());

        // Test edge case: excluded start at the last valid index returns empty sequence
        let sliced = view.range_checked((Bound::Excluded(8), Bound::Unbounded));
        assert!(sliced.is_some());
        assert_eq!(0, sliced.unwrap().len());

        // Test edge case: excluded start that would overflow when adding 1
        let sliced = view.range_checked((Bound::Excluded(usize::MAX), Bound::Unbounded));
        assert!(sliced.is_none());
    }

    #[test]
    fn slice_oob_is_panic() {
        let span_builder = std_alloc_block::allocate(nz!(1000)).into_span_builder();

        let mut buf = BytesBuf::from_span_builders([span_builder]);
        buf.put_byte_repeated(0, 100);

        let view = buf.consume_all();

        assert_panic!(_ = view.range(0..101));
        assert_panic!(_ = view.range(0..=100));
        assert_panic!(_ = view.range(100..=100));
        assert_panic!(_ = view.range(100..101));
        assert_panic!(_ = view.range(101..));
        assert_panic!(_ = view.range(101..101));
        assert_panic!(_ = view.range(101..=101));
    }

    #[test]
    fn slice_at_boundary_is_not_panic() {
        let span_builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        let mut buf = BytesBuf::from_span_builders([span_builder]);
        buf.put_byte_repeated(0, 100);

        let view = buf.consume_all();

        assert_eq!(0, view.range(0..0).len());
        assert_eq!(1, view.range(0..=0).len());
        assert_eq!(0, view.range(..0).len());
        assert_eq!(1, view.range(..=0).len());
        assert_eq!(0, view.range(100..100).len());
        assert_eq!(0, view.range(99..99).len());
        assert_eq!(1, view.range(99..=99).len());
        assert_eq!(1, view.range(99..).len());
        assert_eq!(100, view.range(..).len());
    }

    #[test]
    fn slice_empty_is_empty_if_not_oob() {
        let span_builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        let mut buf = BytesBuf::from_span_builders([span_builder]);

        for i in 0..100 {
            buf.put_byte(i);
        }

        let view = buf.consume_all();

        let sub_sequence = view.range(50..50);
        assert_eq!(0, sub_sequence.len());

        // 100 is the index at the end of the view - still in-bounds, if at edge.
        let sub_sequence = view.range(100..100);
        assert_eq!(0, sub_sequence.len());

        assert!(view.range_checked(101..101).is_none());
    }

    #[test]
    fn consume_all_slices() {
        const SPAN_SIZE: NonZero<BlockSize> = nz!(10);

        // A multi-span sequence, 10 bytes x10.
        let span_builders = iter::repeat_with(|| std_alloc_block::allocate(SPAN_SIZE).into_span_builder())
            .take(10)
            .collect::<Vec<_>>();

        let mut buf = BytesBuf::from_span_builders(span_builders);

        for i in 0..100 {
            buf.put_byte(i);
        }

        let mut view = buf.consume_all();

        let mut slice_index = 0;
        let mut bytes_consumed = 0;

        view.consume_all_slices(|slice| {
            assert_eq!(slice.len(), 10);
            bytes_consumed += slice.len();

            for i in 0..10 {
                assert_eq!(slice_index * 10 + i, slice[i] as usize);
            }

            slice_index += 1;
        });

        assert_eq!(bytes_consumed, 100);

        view.consume_all_slices(|_| unreachable!("view should now be empty"));
    }

    #[test]
    fn multithreaded_usage() {
        fn post_to_another_thread(view: BytesView) {
            thread::spawn(move || {
                let mut view = view;
                assert_eq!(view.get_byte(), b'H');
                assert_eq!(view.get_byte(), b'e');
                assert_eq!(view.get_byte(), b'l');
                assert_eq!(view.get_byte(), b'l');
                assert_eq!(view.get_byte(), b'o');
            })
            .join()
            .unwrap();
        }

        let memory = TransparentTestMemory::new();
        let view = BytesView::copied_from_slice(b"Hello, world!", &memory);

        post_to_another_thread(view);
    }

    #[test]
    fn vectored_read_as_io_slice() {
        let memory = TransparentTestMemory::new();
        let segment1 = BytesView::copied_from_slice(b"Hello, world!", &memory);
        let segment2 = BytesView::copied_from_slice(b"Hello, another world!", &memory);

        let view = BytesView::from_views(vec![segment1.clone(), segment2.clone()]);

        let mut io_slices = vec![];
        let ioslice_count = view.io_slices(&mut io_slices);
        assert_eq!(ioslice_count, 0);

        let mut io_slices = vec![IoSlice::new(&[]); 4];
        let ioslice_count = view.io_slices(&mut io_slices);

        assert_eq!(ioslice_count, 2);
        assert_eq!(io_slices[0].len(), segment1.len());
        assert_eq!(io_slices[1].len(), segment2.len());
    }

    #[test]
    fn vectored_read_as_slice() {
        let memory = TransparentTestMemory::new();
        let segment1 = BytesView::copied_from_slice(b"Hello, world!", &memory);
        let segment2 = BytesView::copied_from_slice(b"Hello, another world!", &memory);

        let view = BytesView::from_views(vec![segment1.clone(), segment2.clone()]);

        let mut slices: Vec<&[u8]> = vec![];
        let slice_count = view.slices(&mut slices);
        assert_eq!(slice_count, 0);

        let mut slices: Vec<&[u8]> = vec![&[]; 4];
        let slice_count = view.slices(&mut slices);

        assert_eq!(slice_count, 2);
        assert_eq!(slices[0].len(), segment1.len());
        assert_eq!(slices[1].len(), segment2.len());
    }

    #[test]
    fn eq_sequence() {
        let memory = TransparentTestMemory::new();

        let view1 = BytesView::copied_from_slice(b"Hello, world!", &memory);
        let view2 = BytesView::copied_from_slice(b"Hello, world!", &memory);

        assert_eq!(view1, view2);

        let view3 = BytesView::copied_from_slice(b"Jello, world!", &memory);

        assert_ne!(view1, view3);

        let view4 = BytesView::copied_from_slice(b"Hello, world! ", &memory);

        assert_ne!(view1, view4);

        let view5_part1 = BytesView::copied_from_slice(b"Hello, ", &memory);
        let view5_part2 = BytesView::copied_from_slice(b"world!", &memory);
        let view5 = BytesView::from_views([view5_part1, view5_part2]);

        assert_eq!(view1, view5);
        assert_ne!(view5, view3);

        let view6 = BytesView::copied_from_slice(b"Hello, ", &memory);

        assert_ne!(view1, view6);
        assert_ne!(view5, view6);
    }

    #[test]
    fn eq_slice() {
        let memory = TransparentTestMemory::new();

        let view1 = BytesView::copied_from_slice(b"Hello, world!", &memory);

        assert_eq!(view1, b"Hello, world!".as_slice());
        assert_ne!(view1, b"Jello, world!".as_slice());
        assert_ne!(view1, b"Hello, world! ".as_slice());

        assert_eq!(b"Hello, world!".as_slice(), view1);
        assert_ne!(b"Jello, world!".as_slice(), view1);
        assert_ne!(b"Hello, world! ".as_slice(), view1);

        let view2_part1 = BytesView::copied_from_slice(b"Hello, ", &memory);
        let view2_part2 = BytesView::copied_from_slice(b"world!", &memory);
        let view2 = BytesView::from_views([view2_part1, view2_part2]);

        assert_eq!(view2, b"Hello, world!".as_slice());
        assert_ne!(view2, b"Jello, world!".as_slice());
        assert_ne!(view2, b"Hello, world! ".as_slice());
        assert_ne!(view2, b"Hello, ".as_slice());

        assert_eq!(b"Hello, world!".as_slice(), view2);
        assert_ne!(b"Jello, world!".as_slice(), view2);
        assert_ne!(b"Hello, world! ".as_slice(), view2);
        assert_ne!(b"Hello, ".as_slice(), view2);
    }

    #[test]
    fn eq_array() {
        let memory = TransparentTestMemory::new();

        let view1 = BytesView::copied_from_slice(b"Hello, world!", &memory);

        assert_eq!(view1, b"Hello, world!");
        assert_ne!(view1, b"Jello, world!");
        assert_ne!(view1, b"Hello, world! ");

        assert_eq!(b"Hello, world!", view1);
        assert_ne!(b"Jello, world!", view1);
        assert_ne!(b"Hello, world! ", view1);

        let view2_part1 = BytesView::copied_from_slice(b"Hello, ", &memory);
        let view2_part2 = BytesView::copied_from_slice(b"world!", &memory);
        let view2 = BytesView::from_views([view2_part1, view2_part2]);

        assert_eq!(view2, b"Hello, world!");
        assert_ne!(view2, b"Jello, world!");
        assert_ne!(view2, b"Hello, world! ");
        assert_ne!(view2, b"Hello, ");

        assert_eq!(b"Hello, world!", view2);
        assert_ne!(b"Jello, world!", view2);
        assert_ne!(b"Hello, world! ", view2);
        assert_ne!(b"Hello, ", view2);
    }

    #[test]
    fn meta_none() {
        let memory = TransparentTestMemory::new();

        let view1 = BytesView::copied_from_slice(b"Hello, ", &memory);
        let view2 = BytesView::copied_from_slice(b"world!", &memory);

        let view = BytesView::from_views([view1, view2]);

        let mut metas_iter = view.iter_slice_metas();

        // We have two chunks, both without metadata.
        assert!(matches!(metas_iter.next(), Some(None)));
        assert!(matches!(metas_iter.next(), Some(None)));
        assert!(metas_iter.next().is_none());
    }

    #[test]
    fn meta_some() {
        struct GreenMeta;
        struct BlueMeta;

        // SAFETY: We are not allowed to drop this until all BlockRef are gone. This is fine
        // because it is dropped at the end of the function, after all BlockRef instances.
        let block1 = unsafe { TestMemoryBlock::new(nz!(100), Some(Box::new(GreenMeta {}))) };
        let block1 = pin!(block1);

        // SAFETY: We are not allowed to drop this until all BlockRef are gone. This is fine
        // because it is dropped at the end of the function, after all BlockRef instances.
        let block2 = unsafe { TestMemoryBlock::new(nz!(100), Some(Box::new(BlueMeta {}))) };
        let block2 = pin!(block2);

        // SAFETY: We guarantee exclusive access to the memory capacity.
        let block1 = unsafe { block1.as_ref().to_block() };
        // SAFETY: We guarantee exclusive access to the memory capacity.
        let block2 = unsafe { block2.as_ref().to_block() };

        let mut buf = BytesBuf::from_blocks([block1, block2]);

        // Add enough bytes to make use of both blocks.
        buf.put_byte_repeated(123, 166);

        let view = buf.consume_all();

        let mut metas_iter = view.iter_slice_metas();

        // NB! There is no requirement that the BytesBuf use the blocks in the order we gave
        // them in. We use white-box knowledge here to know that it actually reverses the order.
        // This behavior may change in a future version - be ready to change the test if so.

        let meta1 = metas_iter.next().expect("should have first block meta");
        assert!(meta1.is_some());
        assert!(meta1.unwrap().is::<BlueMeta>());

        let meta2 = metas_iter.next().expect("should have second block meta");
        assert!(meta2.is_some());
        assert!(meta2.unwrap().is::<GreenMeta>());

        assert!(metas_iter.next().is_none(), "should have no more metas");
    }

    #[test]
    fn append_single_span() {
        let memory = TransparentTestMemory::new();

        // Create two single-span views.
        let mut view1 = BytesView::copied_from_slice(b"Hello, ", &memory);
        let view2 = BytesView::copied_from_slice(b"world!", &memory);

        assert_eq!(view1.len(), 7);
        assert_eq!(view2.len(), 6);

        view1.append(view2);

        assert_eq!(view1.len(), 13);
        assert_eq!(view1, b"Hello, world!");
    }

    #[test]
    fn append_multi_span() {
        let memory = TransparentTestMemory::new();

        // Create two multi-span views (2 spans each)
        let view1_part1 = BytesView::copied_from_slice(b"AAA", &memory);
        let view1_part2 = BytesView::copied_from_slice(b"BBB", &memory);
        let mut view1 = BytesView::from_views([view1_part1, view1_part2]);

        let view2_part1 = BytesView::copied_from_slice(b"CCC", &memory);
        let view2_part2 = BytesView::copied_from_slice(b"DDD", &memory);
        let view2 = BytesView::from_views([view2_part1, view2_part2]);

        assert_eq!(view1.len(), 6);
        assert_eq!(view2.len(), 6);

        view1.append(view2);

        assert_eq!(view1.len(), 12);
        assert_eq!(view1, b"AAABBBCCCDDD");
    }

    #[test]
    fn append_empty_view() {
        let memory = TransparentTestMemory::new();

        let mut view1 = BytesView::copied_from_slice(b"Hello", &memory);
        let view2 = BytesView::new();

        view1.append(view2);
        assert_eq!(view1.len(), 5);
        assert_eq!(view1, b"Hello");

        let mut view3 = BytesView::new();
        let view4 = BytesView::copied_from_slice(b"world", &memory);

        view3.append(view4);
        assert_eq!(view3.len(), 5);
        assert_eq!(view3, b"world");
    }

    #[test]
    fn concat_single_span() {
        let memory = TransparentTestMemory::new();

        // Create two single-span views
        let view1 = BytesView::copied_from_slice(b"Hello, ", &memory);
        let view2 = BytesView::copied_from_slice(b"world!", &memory);

        assert_eq!(view1.len(), 7);
        assert_eq!(view2.len(), 6);

        let view3 = view1.concat(view2);

        // Original view unchanged
        assert_eq!(view1.len(), 7);
        assert_eq!(view1, b"Hello, ");

        // New view contains combined data
        assert_eq!(view3.len(), 13);
        assert_eq!(view3, b"Hello, world!");
    }

    #[test]
    fn concat_multi_span() {
        let memory = TransparentTestMemory::new();

        // Create two multi-span views (2 spans each)
        let view1_part1 = BytesView::copied_from_slice(b"AAA", &memory);
        let view1_part2 = BytesView::copied_from_slice(b"BBB", &memory);
        let view1 = BytesView::from_views([view1_part1, view1_part2]);

        let view2_part1 = BytesView::copied_from_slice(b"CCC", &memory);
        let view2_part2 = BytesView::copied_from_slice(b"DDD", &memory);
        let view2 = BytesView::from_views([view2_part1, view2_part2]);

        assert_eq!(view1.len(), 6);
        assert_eq!(view2.len(), 6);

        let view3 = view1.concat(view2);

        // Original view unchanged
        assert_eq!(view1.len(), 6);
        assert_eq!(view1, b"AAABBB");

        // New view contains combined data
        assert_eq!(view3.len(), 12);
        assert_eq!(view3, b"AAABBBCCCDDD");
    }

    #[test]
    fn concat_empty_views() {
        let memory = TransparentTestMemory::new();

        let view1 = BytesView::copied_from_slice(b"Hello", &memory);
        let view2 = BytesView::new();

        let view3 = view1.concat(view2);
        assert_eq!(view3.len(), 5);
        assert_eq!(view3, b"Hello");

        let view4 = BytesView::new();
        let view5 = BytesView::copied_from_slice(b"world", &memory);

        let view6 = view4.concat(view5);
        assert_eq!(view6.len(), 5);
        assert_eq!(view6, b"world");
    }

    #[test]
    fn size_change_detector() {
        // The point of this is not to say that we expect it to have a specific size but to allow
        // us to easily detect when the size changes and (if we choose to) bless the change.
        // We assume 64-bit pointers - any support for 32-bit is problem for the future.
        assert_eq!(size_of::<BytesView>(), 272);
    }
}
