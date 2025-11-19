// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::io::IoSlice;
use std::marker::PhantomData;
use std::num::NonZero;
use std::ops::{Bound, RangeBounds};
use std::{iter, mem};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use nm::{Event, Magnitude};
use smallvec::SmallVec;

use crate::{BlockSize, MAX_INLINE_SPANS, Memory, MemoryGuard, Span};

/// A sequence of immutable bytes that can be inspected and consumed.
///
/// Note that only the contents of a sequence are immutable - the sequence itself can be
/// mutated in terms of progressively marking its contents as consumed until it becomes empty.
/// The typical mechanism for consuming the contents of a `ByteSequence` is the [`bytes::buf::Buf`][1]
/// trait that it implements.
///
/// To create a `ByteSequence`, use a [`ByteSequenceBuilder`][3] or clone/slice an existing `ByteSequence`.
#[doc = include_str!("../doc/snippets/sequence_memory_layout.md")]
///
/// [1]: https://docs.rs/bytes/latest/bytes/buf/trait.Buf.html
/// [3]: crate::ByteSequenceBuilder
#[derive(Clone, Debug)]
pub struct ByteSequence {
    /// The spans of the sequence, stored in reverse order for efficient consumption
    /// by popping items off the end of the collection.
    spans_reversed: SmallVec<[Span; MAX_INLINE_SPANS]>,

    /// We cache the length so we do not have to recalculate it every time it is queried.
    len: usize,
}

impl ByteSequence {
    /// Returns an empty sequence.
    ///
    /// Use a [`ByteSequenceBuilder`][1] to create a sequence that contains data.
    ///
    /// [1]: crate::ByteSequenceBuilder
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
        SEQUENCE_CREATED_SPANS.with(|x| x.observe(spans_reversed.len()));

        let len = spans_reversed.iter().map(bytes::Buf::remaining).sum();

        Self { spans_reversed, len }
    }

    /// Concatenates a number of spans, yielding a sequence that combines the spans.
    ///
    /// Later changes made to the input spans will not be reflected in the output sequence.
    #[cfg(test)]
    pub(crate) fn from_spans<I>(spans: I) -> Self
    where
        I: IntoIterator<Item = Span>,
        <I as IntoIterator>::IntoIter: iter::DoubleEndedIterator,
    {
        let spans_reversed = spans.into_iter().rev().collect::<SmallVec<_>>();

        Self::from_spans_reversed(spans_reversed)
    }

    /// Concatenates a number of existing sequences, yielding a combined view.
    ///
    /// Later changes made to the input sequences will not be reflected in the output sequence.
    pub fn from_sequences<I>(sequences: I) -> Self
    where
        I: IntoIterator<Item = Self>,
        <I as IntoIterator>::IntoIter: iter::DoubleEndedIterator,
    {
        // Note that this requires the SmallVec to resize on the fly because thanks to the
        // two-level mapping here, there is no usable size hint that lets it know the size in
        // advance. If we had the span count here, we could avoid some allocations.

        // For a given input ABC123.
        let spans_reversed: SmallVec<_> = sequences
            .into_iter()
            // We first reverse the sequences: 123ABC.
            .rev()
            // And from inside each sequence we take the reversed spans: 321CBA.
            .flat_map(|seq| seq.spans_reversed)
            // Which become our final SmallVec of spans. Great success!
            .collect();

        // We can use this to fine-tune the inline span count once we have real-world data.
        SEQUENCE_CREATED_SPANS.with(|x| x.observe(spans_reversed.len()));

        let len = spans_reversed.iter().map(bytes::Buf::remaining).sum();

        Self { spans_reversed, len }
    }

    /// Shorthand to copy a byte slice into a new `ByteSequence`, which is a common operation.
    #[must_use]
    pub fn copy_from_slice(bytes: &[u8], memory_provider: &impl Memory) -> Self {
        let mut buffer = memory_provider.reserve(bytes.len());
        buffer.put_slice(bytes);
        buffer.consume_all()
    }

    pub(crate) fn into_spans_reversed(self) -> SmallVec<[Span; MAX_INLINE_SPANS]> {
        self.spans_reversed
    }

    /// The number of bytes remaining in the sequence.
    #[must_use]
    pub fn len(&self) -> usize {
        // Sanity check.
        debug_assert_eq!(self.len, self.spans_reversed.iter().map(bytes::Buf::remaining).sum::<usize>());

        self.len
    }

    /// Whether the sequence is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Creates a memory guard that extends the lifetime of the memory blocks that provide the
    /// backing memory capacity for this sequence.
    ///
    /// This can be useful when unsafe code is used to reference the contents of a `ByteSequence` and it
    /// is possible to reach a condition where the `ByteSequence` itself no longer exists, even though
    /// the contents are referenced (e.g. because this is happening in non-Rust code).
    pub fn extend_lifetime(&self) -> MemoryGuard {
        MemoryGuard::new(self.spans_reversed.iter().map(Span::block_ref).map(Clone::clone))
    }

    /// Returns a sub-sequence of the sequence without consuming any data in the original.
    ///
    /// The bounds logic only considers data currently present in the sequence.
    /// Any data already consumed is not considered part of the sequence.
    ///
    /// # Panics
    ///
    /// Panics if the provided range is outside the bounds of the sequence.
    #[must_use]
    pub fn slice<R>(&self, range: R) -> Self
    where
        R: RangeBounds<usize>,
    {
        self.slice_checked(range).expect("provided range out of sequence bounds")
    }

    /// Returns a sub-sequence of the sequence without consuming any data in the original,
    /// or `None` if the range is outside the bounds of the sequence.
    ///
    /// The bounds logic only considers data currently present in the sequence.
    /// Any data already consumed is not considered part of the sequence.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    #[expect(clippy::too_many_lines, reason = "acceptable for now")]
    #[cfg_attr(test, mutants::skip)] // Mutations include impossible conditions that we cannot test as well as mutations that are functionally equivalent.
    pub fn slice_checked<R>(&self, range: R) -> Option<Self>
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

    /// Executes a function `f` on each chunk in the sequence, in order,
    /// and marks the entire sequence as consumed.
    pub fn consume_all_chunks<F>(&mut self, mut f: F)
    where
        F: FnMut(&[u8]),
    {
        while !self.is_empty() {
            f(self.chunk());
            self.advance(self.chunk().len());
        }
    }

    /// Consumes the sequence and returns an instance of `Bytes`.
    ///
    /// We do not expose `From<ByteSequence> for Bytes` because this is not guaranteed to be a cheap
    /// operation and may involve data copying, so `.into_bytes()` must be explicitly called to
    /// make the conversion obvious.
    ///
    /// # Performance
    ///
    /// This operation is zero-copy if the sequence is backed by a single consecutive
    /// span of memory.
    ///
    /// If the sequence is backed by multiple spans of memory, the data will be copied
    /// to a new `Bytes` instance backed by memory capacity from the Rust global allocator.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn into_bytes(self) -> Bytes {
        if self.spans_reversed.is_empty() {
            INTO_BYTES_SHARED.with(|x| x.observe(0));

            Bytes::new()
        } else if self.spans_reversed.len() == 1 {
            // We are a single-span sequence, which can always be zero-copy represented.
            INTO_BYTES_SHARED.with(|x| x.observe(self.len()));

            self.spans_reversed.first().expect("we verified there is one span").to_bytes()
        } else {
            // We must copy, as Bytes can only represent consecutive spans of data.
            let mut bytes = BytesMut::with_capacity(self.len());

            for span in self.spans_reversed.iter().rev() {
                bytes.extend_from_slice(span);
            }

            debug_assert_eq!(self.len(), bytes.len());

            INTO_BYTES_COPIED.with(|x| x.observe(self.len()));

            bytes.freeze()
        }
    }

    /// Returns the first consecutive chunk of bytes in the byte sequence.
    ///
    /// There are no guarantees on the length of each chunk. In a non-empty sequence,
    /// each chunk may contain anywhere between 1 byte and all bytes of the sequence.
    ///
    /// Returns an empty slice if the sequence is empty.
    #[cfg_attr(test, mutants::skip)] // Mutating this can cause infinite loops.
    #[must_use]
    pub fn chunk(&self) -> &[u8] {
        self.spans_reversed.last().map_or::<&[u8], _>(&[], |span| span)
    }

    /// Fills an array of `IoSlice` with chunks from the start of the sequence.
    ///
    /// See also [`chunks_as_slices_vectored()`][1] for a version that fills an array of slices
    /// instead of `IoSlice`.
    ///
    /// [1]: Self::chunks_as_slices_vectored
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn chunks_vectored<'a>(&'a self, dst: &mut [IoSlice<'a>]) -> usize {
        if dst.is_empty() {
            return 0;
        }

        // How many chunks can we fill?
        let chunk_count = self.spans_reversed.len().min(dst.len());

        // Note that IoSlice has a length limit of u32::MAX. Our spans are also limited to u32::MAX
        // by memory manager internal limits (MAX_BLOCK_SIZE), so this is safe.
        for (i, span) in self.spans_reversed.iter().rev().take(chunk_count).enumerate() {
            *dst.get_mut(i).expect("guarded by min()") = IoSlice::new(span);
        }

        chunk_count
    }

    /// Fills an array of slices with chunks from the start of the sequence.
    ///
    /// This is equivalent to [`chunks_vectored()`][1] but as regular slices, without the `IoSlice`
    /// wrapper, which can be unnecessary/limiting and is not always desirable.
    ///
    /// [1]: Self::chunks_vectored
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn chunks_as_slices_vectored<'a>(&'a self, dst: &mut [&'a [u8]]) -> usize {
        if dst.is_empty() {
            return 0;
        }

        // How many chunks can we fill?
        let chunk_count = self.spans_reversed.len().min(dst.len());

        for (i, span) in self.spans_reversed.iter().rev().take(chunk_count).enumerate() {
            *dst.get_mut(i).expect("guarded by min()") = span;
        }

        chunk_count
    }

    /// Inspects the metadata of the current chunk of memory referenced by `chunk()`.
    ///
    /// `None` if there is no metadata associated with the chunk or if the sequence is empty.
    #[must_use]
    pub fn chunk_meta(&self) -> Option<&dyn Any> {
        self.spans_reversed.last().and_then(|span| span.block_ref().meta())
    }

    /// Iterates over the metadata of all the chunks in the sequence.
    ///
    /// A chunk is any consecutive span of memory that would be returned by `chunk()` at some point
    /// during the consumption of a [`ByteSequence`].
    ///
    /// You may wish to iterate over the metadata to determine in advance which implementation
    /// strategy to use for a function, depending on what the metadata indicates about the
    /// configuration of the memory blocks backing the byte sequence.
    pub fn iter_chunk_metas(&self) -> ByteSequenceChunkMetasIterator<'_> {
        ByteSequenceChunkMetasIterator::new(self)
    }

    /// Marks the first `count` bytes of the sequence as consumed, dropping them from the sequence.
    ///
    /// # Panics
    ///
    /// Panics if `count` is greater than the number of bytes remaining in the sequence.
    #[cfg_attr(test, mutants::skip)] // Mutating this can cause infinite loops.
    pub fn advance(&mut self, mut count: usize) {
        self.len = self.len.checked_sub(count).expect("attempted to advance past end of sequence");

        while count > 0 {
            let front = self
                .spans_reversed
                .last_mut()
                .expect("logic error - ran out of spans before advancing over their contents");
            let remaining = front.remaining();

            if count < remaining {
                front.advance(count);
                break;
            }

            self.spans_reversed.pop();
            count = count.checked_sub(remaining).expect("already handled count < remaining case");
        }
    }

    /// Appends another byte sequence to the end of this one.
    ///
    /// # Panics
    ///
    /// Panics if the resulting sequence would be larger than `usize::MAX` bytes.
    pub fn append(&mut self, other: Self) {
        self.len = self
            .len
            .checked_add(other.len)
            .expect("attempted to create a byte sequence larger than usize::MAX bytes");

        self.spans_reversed.insert_many(0, other.spans_reversed);
    }

    /// Returns a new byte sequence that concatenates this byte sequence with another.
    ///
    /// # Panics
    ///
    /// Panics if the resulting sequence would be larger than `usize::MAX` bytes.
    #[must_use]
    pub fn concat(&self, other: Self) -> Self {
        let mut new_sequence = self.clone();
        new_sequence.append(other);
        new_sequence
    }
}

impl Default for ByteSequence {
    fn default() -> Self {
        Self::new()
    }
}

impl Buf for ByteSequence {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn remaining(&self) -> usize {
        self.len()
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn chunk(&self) -> &[u8] {
        self.chunk()
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn chunks_vectored<'a>(&'a self, dst: &mut [IoSlice<'a>]) -> usize {
        self.chunks_vectored(dst)
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn advance(&mut self, cnt: usize) {
        self.advance(cnt);
    }
}

impl PartialEq for ByteSequence {
    fn eq(&self, other: &Self) -> bool {
        // We do not care about the structure, only the contents.
        if self.remaining() != other.remaining() {
            return false;
        }

        let mut remaining_bytes = self.remaining();

        // The two sequences may have differently sized spans, so we only compare in steps
        // of the smallest span size offered by either sequences.

        // We clone the sequences to create temporary views that we slide over the contents.
        let mut self_view = self.clone();
        let mut other_view = other.clone();

        while remaining_bytes > 0 {
            let self_chunk = self_view.chunk();
            let other_chunk = other_view.chunk();

            let chunk_size = NonZero::new(self_chunk.len().min(other_chunk.len()))
                .expect("both sequences said there are remaining bytes but we got an empty chunk");

            let self_slice = self_chunk.get(..chunk_size.get()).expect("already checked that remaining > 0");
            let other_slice = other_chunk.get(..chunk_size.get()).expect("already checked that remaining > 0");

            if self_slice != other_slice {
                // Something is different. That is enough for a determination.
                return false;
            }

            // Advance both sequences by the same amount.
            self_view.advance(chunk_size.get());
            other_view.advance(chunk_size.get());

            remaining_bytes = remaining_bytes
                .checked_sub(chunk_size.get())
                .expect("impossible to consume more bytes from the sequences than are remaining");
        }

        debug_assert_eq!(remaining_bytes, 0);
        debug_assert_eq!(self_view.remaining(), 0);
        debug_assert_eq!(other_view.remaining(), 0);

        true
    }
}

impl PartialEq<&[u8]> for ByteSequence {
    fn eq(&self, other: &&[u8]) -> bool {
        let mut other = *other;

        // We do not care about the structure, only the contents.

        if self.remaining() != other.len() {
            return false;
        }

        let mut remaining_bytes = self.remaining();

        // We clone the sequence to create a temporary view that we slide over the contents.
        let mut self_view = self.clone();

        while remaining_bytes > 0 {
            let self_chunk = self_view.chunk();
            let chunk_size =
                NonZero::new(self_chunk.len()).expect("both sequences said there are remaining bytes but we got an empty chunk");

            let self_slice = self_chunk.get(..chunk_size.get()).expect("already checked that remaining > 0");
            let other_slice = other.get(..chunk_size.get()).expect("already checked that remaining > 0");

            if self_slice != other_slice {
                // Something is different. That is enough for a determination.
                return false;
            }

            // Advance the sequence by the same amount.
            self_view.advance(chunk_size.get());
            other = other.get(chunk_size.get()..).expect("guarded by min() above");

            remaining_bytes = remaining_bytes
                .checked_sub(chunk_size.get())
                .expect("impossible to consume more bytes from the sequences than are remaining");
        }

        debug_assert_eq!(remaining_bytes, 0);
        debug_assert_eq!(self_view.remaining(), 0);
        debug_assert_eq!(other.len(), 0);

        true
    }
}

impl PartialEq<ByteSequence> for &[u8] {
    fn eq(&self, other: &ByteSequence) -> bool {
        other.eq(self)
    }
}

impl<const LEN: usize> PartialEq<&[u8; LEN]> for ByteSequence {
    fn eq(&self, other: &&[u8; LEN]) -> bool {
        self.eq(&other.as_slice())
    }
}

impl<const LEN: usize> PartialEq<ByteSequence> for &[u8; LEN] {
    fn eq(&self, other: &ByteSequence) -> bool {
        other.eq(&self.as_slice())
    }
}

/// Iterator over all `chunk_meta()` results that a [`ByteSequence`] may return.
///
/// Returned by [`ByteSequence::iter_chunk_metas()`][ByteSequence::iter_chunk_metas] and allows you to
/// inspect the metadata of each chunk in the sequence without first consuming previous chunks.
#[must_use]
#[derive(Debug)]
pub struct ByteSequenceChunkMetasIterator<'s> {
    // This starts off as a clone of the parent sequence, just for ease of implementation.
    // We consume the parts of the sequence we have already iterated over.
    sequence: ByteSequence,

    // We keep a reference to the sequence we are iterating over, even though
    // the current implementation does not use it (because a future one might).
    _parent: PhantomData<&'s ByteSequence>,
}

impl<'s> ByteSequenceChunkMetasIterator<'s> {
    pub(crate) fn new(sequence: &'s ByteSequence) -> Self {
        Self {
            sequence: sequence.clone(),
            _parent: PhantomData,
        }
    }
}

impl<'s> Iterator for ByteSequenceChunkMetasIterator<'s> {
    type Item = Option<&'s dyn Any>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.sequence.is_empty() {
            return None;
        }

        let meta = self.sequence.chunk_meta();

        // SAFETY: It is normally not possible to return a self-reference from an iterator because
        // next() only has an implicit lifetime for `&self`, which cannot be named in `Item`.
        // However, we can take advantage of the fact that a `BlockRef` implementation is required
        // to guarantee that the metadata lives as long as any clone of the memory block. Because
        // the iterator has borrowed the parent `ByteSequence` we know that the memory block must live
        // for as long as the iterator lives.
        //
        // Therefor we can just re-stamp the return value with the 's lifetime to indicate that it
        // is valid for as long as the iterator has borrowed the parent ByteSequence for.
        let meta_with_s = unsafe { mem::transmute::<Option<&dyn Any>, Option<&'s dyn Any>>(meta) };

        // Seek forward to the next chunk before we return.
        self.sequence.advance(self.sequence.chunk().len());

        Some(meta_with_s)
    }
}

const SPAN_COUNT_BUCKETS: &[Magnitude] = &[0, 1, 2, 4, 8, 16, 32];

thread_local! {
    static SEQUENCE_CREATED_SPANS: Event = Event::builder()
        .name("sequence_created_spans")
        .histogram(SPAN_COUNT_BUCKETS)
        .build();

    static INTO_BYTES_SHARED: Event = Event::builder()
        .name("sequence_into_bytes_shared")
        .build();

    static INTO_BYTES_COPIED: Event = Event::builder()
        .name("sequence_into_bytes_copied")
        .build();
}

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
    use crate::{ByteSequenceBuilder, TransparentTestMemory, std_alloc_block};

    #[test]
    fn smoke_test() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        builder.put_u64(1234);
        builder.put_u16(16);

        let span1 = builder.consume(nz!(4));
        let span2 = builder.consume(nz!(3));
        let span3 = builder.consume(nz!(3));

        assert_eq!(0, builder.remaining_mut());
        assert_eq!(span1.remaining(), 4);
        assert_eq!(span2.remaining(), 3);
        assert_eq!(span3.remaining(), 3);

        let mut sequence = ByteSequence::from_spans(vec![span1, span2, span3]);

        assert!(!sequence.is_empty());
        assert_eq!(10, sequence.remaining());

        let slice = sequence.chunk();
        assert_eq!(4, slice.len());

        // We read 8 bytes here, so should land straight inside span3.
        assert_eq!(sequence.get_u64(), 1234);

        assert_eq!(2, sequence.remaining());

        let slice = sequence.chunk();
        assert_eq!(2, slice.len());

        assert_eq!(sequence.get_u16(), 16);

        assert_eq!(0, sequence.remaining());
        assert!(sequence.is_empty());
    }

    #[test]
    fn oob_is_panic() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        builder.put_u64(1234);
        builder.put_u16(16);

        let span1 = builder.consume(nz!(4));
        let span2 = builder.consume(nz!(3));
        let span3 = builder.consume(nz!(3));

        let mut sequence = ByteSequence::from_spans(vec![span1, span2, span3]);

        assert_eq!(10, sequence.remaining());

        assert_eq!(sequence.get_u64(), 1234);
        assert_panic!(_ = sequence.get_u32()); // Reads 4 but only has 2 remaining.
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
            let mut builder1 = unsafe { block1.as_ref().to_block() }.into_span_builder();
            // SAFETY: We guarantee exclusive access to the memory capacity.
            let mut builder2 = unsafe { block2.as_ref().to_block() }.into_span_builder();

            builder1.put_u64(1234);
            builder2.put_u64(1234);

            let span1 = builder1.consume(nz!(8));
            let span2 = builder2.consume(nz!(8));

            let sequence = ByteSequence::from_spans(vec![span1, span2]);

            sequence.extend_lifetime()
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
    fn from_sequences() {
        let mut builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        builder.put_u64(1234);
        builder.put_u64(5678);

        let span1 = builder.consume(nz!(8));
        let span2 = builder.consume(nz!(8));

        let sequence1 = ByteSequence::from_spans(vec![span1]);
        let sequence2 = ByteSequence::from_spans(vec![span2]);

        let mut combined = ByteSequence::from_sequences(vec![sequence1, sequence2]);

        assert_eq!(16, combined.remaining());

        assert_eq!(combined.get_u64(), 1234);
        assert_eq!(combined.get_u64(), 5678);
    }

    #[test]
    fn empty_sequence() {
        let sequence = ByteSequence::default();

        assert!(sequence.is_empty());
        assert_eq!(0, sequence.remaining());
        assert_eq!(0, sequence.chunk().len());

        let bytes = sequence.into_bytes();
        assert_eq!(0, bytes.len());
    }

    #[test]
    fn into_bytes() {
        let mut builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        builder.put_u64(1234);
        builder.put_u64(5678);

        let span1 = builder.consume(nz!(8));
        let span2 = builder.consume(nz!(8));

        let sequence_single_span = ByteSequence::from_spans(vec![span1.clone()]);
        let sequence_multi_span = ByteSequence::from_spans(vec![span1, span2]);

        let mut bytes = sequence_single_span.clone().into_bytes();
        assert_eq!(8, bytes.len());
        assert_eq!(1234, bytes.get_u64());

        let mut bytes = sequence_single_span.into_bytes();
        assert_eq!(8, bytes.len());
        assert_eq!(1234, bytes.get_u64());

        let mut bytes = sequence_multi_span.into_bytes();
        assert_eq!(16, bytes.len());
        assert_eq!(1234, bytes.get_u64());
        assert_eq!(5678, bytes.get_u64());
    }

    #[test]
    fn thread_safe_type() {
        assert_impl_all!(ByteSequence: Send, Sync);
    }

    #[test]
    fn slice_from_single_span_sequence() {
        // A very simple sequence to start with, consisting of just one 100 byte span.
        let span_builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        let mut sb = ByteSequenceBuilder::from_span_builders([span_builder]);

        for i in 0..100 {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "manually validated range of values is safe"
            )]
            sb.put_u8(i as u8);
        }

        let sequence = sb.consume_all();

        let mut sub_sequence = sequence.slice(50..55);

        assert_eq!(5, sub_sequence.len());
        assert_eq!(100, sequence.len());

        assert_eq!(50, sub_sequence.get_u8());

        assert_eq!(4, sub_sequence.len());
        assert_eq!(100, sequence.len());

        assert_eq!(51, sub_sequence.get_u8());
        assert_eq!(52, sub_sequence.get_u8());
        assert_eq!(53, sub_sequence.get_u8());
        assert_eq!(54, sub_sequence.get_u8());

        assert_eq!(0, sub_sequence.len());

        assert!(sequence.slice_checked(0..101).is_none());
        assert!(sequence.slice_checked(100..101).is_none());
        assert!(sequence.slice_checked(101..101).is_none());
    }

    #[test]
    fn slice_from_multi_span_sequence() {
        const SPAN_SIZE: NonZero<BlockSize> = nz!(10);

        // A multi-span sequence, 10 bytes x10.
        let span_builders = iter::repeat_with(|| std_alloc_block::allocate(SPAN_SIZE).into_span_builder())
            .take(10)
            .collect::<Vec<_>>();

        let mut sb = ByteSequenceBuilder::from_span_builders(span_builders);

        for i in 0..100 {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "manually validated range of values is safe"
            )]
            sb.put_u8(i as u8);
        }

        let sequence = sb.consume_all();

        let mut first5 = sequence.slice(0..5);
        assert_eq!(5, first5.len());
        assert_eq!(100, sequence.len());
        assert_eq!(0, first5.get_u8());

        let mut last5 = sequence.slice(95..100);
        assert_eq!(5, last5.len());
        assert_eq!(100, sequence.len());
        assert_eq!(95, last5.get_u8());

        let mut middle5 = sequence.slice(49..54);
        assert_eq!(5, middle5.len());
        assert_eq!(100, sequence.len());
        assert_eq!(49, middle5.get_u8());
        assert_eq!(50, middle5.get_u8());
        assert_eq!(51, middle5.get_u8());
        assert_eq!(52, middle5.get_u8());
        assert_eq!(53, middle5.get_u8());

        assert!(sequence.slice_checked(0..101).is_none());
        assert!(sequence.slice_checked(100..101).is_none());
        assert!(sequence.slice_checked(101..101).is_none());
    }

    #[test]
    fn slice_indexing_kinds() {
        let span_builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        let mut sb = ByteSequenceBuilder::from_span_builders([span_builder]);
        sb.put_u8(0);
        sb.put_u8(1);
        sb.put_u8(2);
        sb.put_u8(3);
        sb.put_u8(4);
        sb.put_u8(5);

        let sequence = sb.consume_all();

        let mut middle_four = sequence.slice(1..5);
        assert_eq!(4, middle_four.len());
        assert_eq!(1, middle_four.get_u8());
        assert_eq!(2, middle_four.get_u8());
        assert_eq!(3, middle_four.get_u8());
        assert_eq!(4, middle_four.get_u8());

        let mut middle_four = sequence.slice(1..=4);
        assert_eq!(4, middle_four.len());
        assert_eq!(1, middle_four.get_u8());
        assert_eq!(2, middle_four.get_u8());
        assert_eq!(3, middle_four.get_u8());
        assert_eq!(4, middle_four.get_u8());

        let mut last_two = sequence.slice(4..);
        assert_eq!(2, last_two.len());
        assert_eq!(4, last_two.get_u8());
        assert_eq!(5, last_two.get_u8());

        let mut first_two = sequence.slice(..2);
        assert_eq!(2, first_two.len());
        assert_eq!(0, first_two.get_u8());
        assert_eq!(1, first_two.get_u8());

        let mut first_two = sequence.slice(..=1);
        assert_eq!(2, first_two.len());
        assert_eq!(0, first_two.get_u8());
        assert_eq!(1, first_two.get_u8());
    }

    #[test]
    fn slice_checked_with_excluded_start_bound() {
        use std::ops::Bound;

        let span_builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        let mut sb = ByteSequenceBuilder::from_span_builders([span_builder]);
        sb.put_u8(0);
        sb.put_u8(1);
        sb.put_u8(2);
        sb.put_u8(3);
        sb.put_u8(4);
        sb.put_u8(5);
        sb.put_u8(6);
        sb.put_u8(7);
        sb.put_u8(8);

        let sequence = sb.consume_all();

        // Test with excluded start bound: (Bound::Excluded(1), Bound::Excluded(5))
        // This should be equivalent to 2..5 (items at indices 2, 3, 4)
        let sliced = sequence.slice_checked((Bound::Excluded(1), Bound::Excluded(5)));
        assert!(sliced.is_some());
        let mut sliced = sliced.unwrap();
        assert_eq!(3, sliced.len());
        assert_eq!(2, sliced.get_u8());
        assert_eq!(3, sliced.get_u8());
        assert_eq!(4, sliced.get_u8());

        // Test edge case: excluded start at the last valid index returns empty sequence
        let sliced = sequence.slice_checked((Bound::Excluded(8), Bound::Unbounded));
        assert!(sliced.is_some());
        assert_eq!(0, sliced.unwrap().len());

        // Test edge case: excluded start that would overflow when adding 1
        let sliced = sequence.slice_checked((Bound::Excluded(usize::MAX), Bound::Unbounded));
        assert!(sliced.is_none());
    }

    #[test]
    fn slice_oob_is_panic() {
        let span_builder = std_alloc_block::allocate(nz!(1000)).into_span_builder();

        let mut sb = ByteSequenceBuilder::from_span_builders([span_builder]);
        sb.put_bytes(0, 100);

        let sequence = sb.consume_all();

        assert_panic!(_ = sequence.slice(0..101));
        assert_panic!(_ = sequence.slice(0..=100));
        assert_panic!(_ = sequence.slice(100..=100));
        assert_panic!(_ = sequence.slice(100..101));
        assert_panic!(_ = sequence.slice(101..));
        assert_panic!(_ = sequence.slice(101..101));
        assert_panic!(_ = sequence.slice(101..=101));
    }

    #[test]
    fn slice_at_boundary_is_not_panic() {
        let span_builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        let mut sb = ByteSequenceBuilder::from_span_builders([span_builder]);
        sb.put_bytes(0, 100);

        let sequence = sb.consume_all();

        assert_eq!(0, sequence.slice(0..0).len());
        assert_eq!(1, sequence.slice(0..=0).len());
        assert_eq!(0, sequence.slice(..0).len());
        assert_eq!(1, sequence.slice(..=0).len());
        assert_eq!(0, sequence.slice(100..100).len());
        assert_eq!(0, sequence.slice(99..99).len());
        assert_eq!(1, sequence.slice(99..=99).len());
        assert_eq!(1, sequence.slice(99..).len());
        assert_eq!(100, sequence.slice(..).len());
    }

    #[test]
    fn slice_empty_is_empty_if_not_oob() {
        let span_builder = std_alloc_block::allocate(nz!(100)).into_span_builder();

        let mut sb = ByteSequenceBuilder::from_span_builders([span_builder]);

        for i in 0..100 {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "manually validated range of values is safe"
            )]
            sb.put_u8(i as u8);
        }

        let sequence = sb.consume_all();

        let sub_sequence = sequence.slice(50..50);
        assert_eq!(0, sub_sequence.len());

        // 100 is the index at the end of the sequence - still in-bounds, if at edge.
        let sub_sequence = sequence.slice(100..100);
        assert_eq!(0, sub_sequence.len());

        assert!(sequence.slice_checked(101..101).is_none());
    }

    #[test]
    fn consume_all_chunks() {
        const SPAN_SIZE: NonZero<BlockSize> = nz!(10);

        // A multi-span sequence, 10 bytes x10.
        let span_builders = iter::repeat_with(|| std_alloc_block::allocate(SPAN_SIZE).into_span_builder())
            .take(10)
            .collect::<Vec<_>>();

        let mut sb = ByteSequenceBuilder::from_span_builders(span_builders);

        for i in 0..100 {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "manually validated range of values is safe"
            )]
            sb.put_u8(i as u8);
        }

        let mut sequence = sb.consume_all();

        let mut chunk_index = 0;
        let mut bytes_consumed = 0;

        sequence.consume_all_chunks(|chunk| {
            assert_eq!(chunk.len(), 10);
            bytes_consumed += chunk.len();

            for i in 0..10 {
                assert_eq!(chunk_index * 10 + i, chunk[i] as usize);
            }

            chunk_index += 1;
        });

        assert_eq!(bytes_consumed, 100);

        sequence.consume_all_chunks(|_| unreachable!("sequence should now be empty"));
    }

    #[test]
    fn multithreaded_usage() {
        fn post_to_another_thread(s: ByteSequence) {
            thread::spawn(move || {
                let mut s = s;
                assert_eq!(s.get_u8(), b'H');
                assert_eq!(s.get_u8(), b'e');
                assert_eq!(s.get_u8(), b'l');
                assert_eq!(s.get_u8(), b'l');
                assert_eq!(s.get_u8(), b'o');
            })
            .join()
            .unwrap();
        }

        let memory = TransparentTestMemory::new();
        let s = ByteSequence::copy_from_slice(b"Hello, world!", &memory);

        post_to_another_thread(s);
    }

    #[test]
    fn vectored_read_as_io_slice() {
        let memory = TransparentTestMemory::new();
        let segment1 = ByteSequence::copy_from_slice(b"Hello, world!", &memory);
        let segment2 = ByteSequence::copy_from_slice(b"Hello, another world!", &memory);

        let sequence = ByteSequence::from_sequences(vec![segment1.clone(), segment2.clone()]);

        let mut io_slices = vec![];
        let ioslice_count = Buf::chunks_vectored(&sequence, &mut io_slices);
        assert_eq!(ioslice_count, 0);

        let mut io_slices = vec![IoSlice::new(&[]); 4];
        let ioslice_count = Buf::chunks_vectored(&sequence, &mut io_slices);

        assert_eq!(ioslice_count, 2);
        assert_eq!(io_slices[0].len(), segment1.len());
        assert_eq!(io_slices[1].len(), segment2.len());
    }

    #[test]
    fn vectored_read_as_slice() {
        let memory = TransparentTestMemory::new();
        let segment1 = ByteSequence::copy_from_slice(b"Hello, world!", &memory);
        let segment2 = ByteSequence::copy_from_slice(b"Hello, another world!", &memory);

        let sequence = ByteSequence::from_sequences(vec![segment1.clone(), segment2.clone()]);

        let mut slices: Vec<&[u8]> = vec![];
        let slice_count = sequence.chunks_as_slices_vectored(&mut slices);
        assert_eq!(slice_count, 0);

        let mut slices: Vec<&[u8]> = vec![&[]; 4];
        let slice_count = sequence.chunks_as_slices_vectored(&mut slices);

        assert_eq!(slice_count, 2);
        assert_eq!(slices[0].len(), segment1.len());
        assert_eq!(slices[1].len(), segment2.len());
    }

    #[test]
    fn eq_sequence() {
        let memory = TransparentTestMemory::new();

        let s1 = ByteSequence::copy_from_slice(b"Hello, world!", &memory);
        let s2 = ByteSequence::copy_from_slice(b"Hello, world!", &memory);

        assert_eq!(s1, s2);

        let s3 = ByteSequence::copy_from_slice(b"Jello, world!", &memory);

        assert_ne!(s1, s3);

        let s4 = ByteSequence::copy_from_slice(b"Hello, world! ", &memory);

        assert_ne!(s1, s4);

        let s5_part1 = ByteSequence::copy_from_slice(b"Hello, ", &memory);
        let s5_part2 = ByteSequence::copy_from_slice(b"world!", &memory);
        let s5 = ByteSequence::from_sequences([s5_part1, s5_part2]);

        assert_eq!(s1, s5);
        assert_ne!(s5, s3);

        let s6 = ByteSequence::copy_from_slice(b"Hello, ", &memory);

        assert_ne!(s1, s6);
        assert_ne!(s5, s6);
    }

    #[test]
    fn eq_slice() {
        let memory = TransparentTestMemory::new();

        let s1 = ByteSequence::copy_from_slice(b"Hello, world!", &memory);

        assert_eq!(s1, b"Hello, world!".as_slice());
        assert_ne!(s1, b"Jello, world!".as_slice());
        assert_ne!(s1, b"Hello, world! ".as_slice());

        assert_eq!(b"Hello, world!".as_slice(), s1);
        assert_ne!(b"Jello, world!".as_slice(), s1);
        assert_ne!(b"Hello, world! ".as_slice(), s1);

        let s2_part1 = ByteSequence::copy_from_slice(b"Hello, ", &memory);
        let s2_part2 = ByteSequence::copy_from_slice(b"world!", &memory);
        let s2 = ByteSequence::from_sequences([s2_part1, s2_part2]);

        assert_eq!(s2, b"Hello, world!".as_slice());
        assert_ne!(s2, b"Jello, world!".as_slice());
        assert_ne!(s2, b"Hello, world! ".as_slice());
        assert_ne!(s2, b"Hello, ".as_slice());

        assert_eq!(b"Hello, world!".as_slice(), s2);
        assert_ne!(b"Jello, world!".as_slice(), s2);
        assert_ne!(b"Hello, world! ".as_slice(), s2);
        assert_ne!(b"Hello, ".as_slice(), s2);
    }

    #[test]
    fn eq_array() {
        let memory = TransparentTestMemory::new();

        let s1 = ByteSequence::copy_from_slice(b"Hello, world!", &memory);

        assert_eq!(s1, b"Hello, world!");
        assert_ne!(s1, b"Jello, world!");
        assert_ne!(s1, b"Hello, world! ");

        assert_eq!(b"Hello, world!", s1);
        assert_ne!(b"Jello, world!", s1);
        assert_ne!(b"Hello, world! ", s1);

        let s2_part1 = ByteSequence::copy_from_slice(b"Hello, ", &memory);
        let s2_part2 = ByteSequence::copy_from_slice(b"world!", &memory);
        let s2 = ByteSequence::from_sequences([s2_part1, s2_part2]);

        assert_eq!(s2, b"Hello, world!");
        assert_ne!(s2, b"Jello, world!");
        assert_ne!(s2, b"Hello, world! ");
        assert_ne!(s2, b"Hello, ");

        assert_eq!(b"Hello, world!", s2);
        assert_ne!(b"Jello, world!", s2);
        assert_ne!(b"Hello, world! ", s2);
        assert_ne!(b"Hello, ", s2);
    }

    #[test]
    fn meta_none() {
        let memory = TransparentTestMemory::new();

        let s1 = ByteSequence::copy_from_slice(b"Hello, ", &memory);
        let s2 = ByteSequence::copy_from_slice(b"world!", &memory);

        let s = ByteSequence::from_sequences([s1, s2]);

        let mut metas_iter = s.iter_chunk_metas();

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

        let mut builder = ByteSequenceBuilder::from_blocks([block1, block2]);

        // Add enough bytes to make use of both blocks.
        builder.put_bytes(123, 166);

        let s = builder.consume_all();

        let mut metas_iter = s.iter_chunk_metas();

        // NB! There is no requirement that the ByteSequenceBuilder use the blocks in the order we gave
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

        // Create two single-span sequences
        let mut s1 = ByteSequence::copy_from_slice(b"Hello, ", &memory);
        let s2 = ByteSequence::copy_from_slice(b"world!", &memory);

        assert_eq!(s1.len(), 7);
        assert_eq!(s2.len(), 6);

        s1.append(s2);

        assert_eq!(s1.len(), 13);
        assert_eq!(s1, b"Hello, world!");
    }

    #[test]
    fn append_multi_span() {
        let memory = TransparentTestMemory::new();

        // Create two multi-span sequences (2 spans each)
        let s1_part1 = ByteSequence::copy_from_slice(b"AAA", &memory);
        let s1_part2 = ByteSequence::copy_from_slice(b"BBB", &memory);
        let mut s1 = ByteSequence::from_sequences([s1_part1, s1_part2]);

        let s2_part1 = ByteSequence::copy_from_slice(b"CCC", &memory);
        let s2_part2 = ByteSequence::copy_from_slice(b"DDD", &memory);
        let s2 = ByteSequence::from_sequences([s2_part1, s2_part2]);

        assert_eq!(s1.len(), 6);
        assert_eq!(s2.len(), 6);

        s1.append(s2);

        assert_eq!(s1.len(), 12);
        assert_eq!(s1, b"AAABBBCCCDDD");
    }

    #[test]
    fn append_empty_sequences() {
        let memory = TransparentTestMemory::new();

        let mut s1 = ByteSequence::copy_from_slice(b"Hello", &memory);
        let s2 = ByteSequence::new();

        s1.append(s2);
        assert_eq!(s1.len(), 5);
        assert_eq!(s1, b"Hello");

        let mut s3 = ByteSequence::new();
        let s4 = ByteSequence::copy_from_slice(b"world", &memory);

        s3.append(s4);
        assert_eq!(s3.len(), 5);
        assert_eq!(s3, b"world");
    }

    #[test]
    fn concat_single_span() {
        let memory = TransparentTestMemory::new();

        // Create two single-span sequences
        let s1 = ByteSequence::copy_from_slice(b"Hello, ", &memory);
        let s2 = ByteSequence::copy_from_slice(b"world!", &memory);

        assert_eq!(s1.len(), 7);
        assert_eq!(s2.len(), 6);

        let s3 = s1.concat(s2);

        // Original sequences unchanged
        assert_eq!(s1.len(), 7);
        assert_eq!(s1, b"Hello, ");

        // New sequence contains combined data
        assert_eq!(s3.len(), 13);
        assert_eq!(s3, b"Hello, world!");
    }

    #[test]
    fn concat_multi_span() {
        let memory = TransparentTestMemory::new();

        // Create two multi-span sequences (2 spans each)
        let s1_part1 = ByteSequence::copy_from_slice(b"AAA", &memory);
        let s1_part2 = ByteSequence::copy_from_slice(b"BBB", &memory);
        let s1 = ByteSequence::from_sequences([s1_part1, s1_part2]);

        let s2_part1 = ByteSequence::copy_from_slice(b"CCC", &memory);
        let s2_part2 = ByteSequence::copy_from_slice(b"DDD", &memory);
        let s2 = ByteSequence::from_sequences([s2_part1, s2_part2]);

        assert_eq!(s1.len(), 6);
        assert_eq!(s2.len(), 6);

        let s3 = s1.concat(s2);

        // Original sequences unchanged
        assert_eq!(s1.len(), 6);
        assert_eq!(s1, b"AAABBB");

        // New sequence contains combined data
        assert_eq!(s3.len(), 12);
        assert_eq!(s3, b"AAABBBCCCDDD");
    }

    #[test]
    fn concat_empty_sequences() {
        let memory = TransparentTestMemory::new();

        let s1 = ByteSequence::copy_from_slice(b"Hello", &memory);
        let s2 = ByteSequence::new();

        let s3 = s1.concat(s2);
        assert_eq!(s3.len(), 5);
        assert_eq!(s3, b"Hello");

        let s4 = ByteSequence::new();
        let s5 = ByteSequence::copy_from_slice(b"world", &memory);

        let s6 = s4.concat(s5);
        assert_eq!(s6.len(), 5);
        assert_eq!(s6, b"world");
    }

    #[test]
    fn size_change_detector() {
        // The point of this is not to say that we expect it to have a specific size but to allow
        // us to easily detect when the size changes and (if we choose to) bless the change.
        // We assume 64-bit pointers - any support for 32-bit is problem for the future.
        assert_eq!(size_of::<ByteSequence>(), 272);
    }
}
