// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::io::IoSlice;
use std::ops::{Bound, RangeBounds};

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::{MemoryGuard, ProvideMemory, Span};

/// A sequence of immutable bytes that can be inspected and consumed.
///
/// Note that only the contents of a sequence are immutable - the sequence itself can be
/// mutated in terms of progressively marking its contents as consumed until it becomes empty.
/// The typical mechanism for consuming the contents of a `Sequence` is the [`bytes::buf::Buf`][1]
/// trait that it implements.
///
/// This type is designed for operating only on data stored in memory owned by the I/O subsystem
/// and is not a general-purpose "sequence of bytes" abstraction. For general-purpose use, consider
/// the `bytes::Bytes` type.
///
/// To create a `Sequence`, use a [`SequenceBuilder`][3] or clone/slice an existing `Sequence`.
///
#[doc = include_str!("../doc/snippets/sequence_memory_layout.md")]
///
/// [1]: https://docs.rs/bytes/latest/bytes/buf/trait.Buf.html
/// [3]: crate::SequenceBuilder
#[derive(Clone, Debug)]
pub struct Sequence {
    // TODO: Eliminate the dynamic allocation here, at least for typical cases.
    spans: VecDeque<Span>,
}

impl Sequence {
    /// Returns an empty sequence.
    ///
    /// Use a [`SequenceBuilder`][1] to create a sequence that contains data.
    ///
    /// [1]: crate::SequenceBuilder
    #[cfg_attr(test, mutants::skip)] // Generates no-op mutations, not useful.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            spans: VecDeque::new(),
        }
    }

    /// Concatenates a number of spans, yielding a sequence that combines the spans.
    ///
    /// Later changes made to the input spans will not be reflected in the output sequence.
    pub(crate) fn from_spans(spans: impl IntoIterator<Item = Span>) -> Self {
        Self {
            spans: VecDeque::from_iter(spans),
        }
    }

    /// Concatenates a number of existing sequences, yielding a combined view.
    ///
    /// Later changes made to the input sequences will not be reflected in the output sequence.
    ///
    /// # Panics
    ///
    /// Panics if the input sequences reference memory owned by different I/O drivers. I/O memory
    /// cannot be shared between different I/O drivers.
    pub fn from_sequences(sequences: impl IntoIterator<Item = Self>) -> Self {
        Self {
            spans: sequences
                .into_iter()
                .flat_map(|seq| seq.spans.into_iter())
                .collect(),
        }
    }

    /// Creates a new sequence from a `Bytes` instance.
    pub fn from_bytes(bytes: impl Into<Bytes>, memory: &impl ProvideMemory) -> Self {
        let bytes = bytes.into();
        let mut builder = memory.reserve(bytes.len());
        builder.put_slice(&bytes);
        builder.consume_all()
    }

    pub(crate) fn into_spans(self) -> VecDeque<Span> {
        self.spans
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.remaining()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Creates a memory guard that extends the lifetime of the I/O blocks that provide the backing
    /// memory for this sequence.
    ///
    /// This is used to ensure that the I/O blocks are not reused during an I/O operation even if
    /// the originator of the operation drops all `Sequence` and `Span` instances, making the block
    /// unreachable from Rust code.
    pub fn extend_lifetime(&self) -> MemoryGuard {
        MemoryGuard::new(self.spans.iter().map(Span::block).map(Clone::clone))
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
        self.slice_checked(range)
            .expect("provided range out of sequence bounds")
    }

    /// Returns a sub-sequence of the sequence without consuming any data in the original,
    /// or `None` if the range is outside the bounds of the sequence.
    ///
    /// The bounds logic only considers data currently present in the sequence.
    /// Any data already consumed is not considered part of the sequence.
    ///
    /// # Panics
    ///
    /// TODO: Document panics
    #[must_use]
    pub fn slice_checked<R>(&self, range: R) -> Option<Self>
    where
        R: RangeBounds<usize>,
    {
        let mut remaining_until_range = match range.start_bound() {
            Bound::Included(&x) => x,
            Bound::Excluded(&x) => x.checked_add(1)?,
            Bound::Unbounded => 0,
        };

        let mut remaining_in_range = match range.end_bound() {
            Bound::Included(&x) => x.checked_add(1)?.checked_sub(remaining_until_range)?,
            Bound::Excluded(&x) => x.checked_sub(remaining_until_range)?,
            Bound::Unbounded => self.len().checked_sub(remaining_until_range)?,
        };

        let required_len = remaining_until_range
            .checked_add(remaining_in_range)
            .expect("overflowing usize is impossible because we are calculating offset into usize-bounded range");

        if required_len > self.len() {
            // Did not have enough data to cover the range.
            return None;
        }

        // We collect the spans that will form the new sequence in here.
        let mut spans = VecDeque::new();

        for span in &self.spans {
            // If we have enough spans to construct a sequence over the range, we can stop.
            if remaining_in_range == 0 {
                break;
            }

            if remaining_until_range > span.remaining() {
                // We are not yet in the range and can skip this entire span.
                remaining_until_range = remaining_until_range
                    .checked_sub(span.len())
                    .expect("somehow ended up with negative bytes remaining until range start - only possible if the math is wrong");
            } else if remaining_until_range > 0 {
                // We are not yet in the range but the start of the range is inside this span.
                let remaining_in_span = span.len()
                    .checked_sub(remaining_until_range)
                    .expect("somehow ended up with negative bytes remaining in span - only possible if the math is wrong");

                let bytes_to_take = remaining_in_span.min(remaining_in_range);

                let remaining_span_end = remaining_until_range
                    .checked_add(bytes_to_take)
                    .expect("overflowing usize is impossible because we are calculating offset into usize-bounded span");

                let remaining_span = span.slice(remaining_until_range..remaining_span_end);
                remaining_until_range = 0;
                remaining_in_range = remaining_in_range
                    .checked_sub(bytes_to_take)
                    .expect("somehow ended up with negative bytes remaining in range - only possible if the math is wrong");

                spans.push_back(remaining_span);
            } else {
                // We are already in the range, so can take part or all of the span.
                let bytes_to_take = span.len().min(remaining_in_range);

                let remaining_span_end = remaining_until_range
                    .checked_add(bytes_to_take)
                    .expect("overflowing usize is impossible because we are calculating offset into usize-bounded span");

                let remaining_span = span.slice(0..remaining_span_end);
                remaining_in_range = remaining_in_range
                    .checked_sub(bytes_to_take)
                    .expect("somehow ended up with negative bytes remaining in range - only possible if the math is wrong");

                spans.push_back(remaining_span);
            }
        }

        assert_eq!(
            remaining_in_range, 0,
            "we verified on entry that we have enough data to cover the range"
        );

        Some(Self::from_spans(spans))
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

    /// Consumes the sequence and returns an instance of `Bytes`. This operation is zero-copy if the sequence
    /// is backed by a single consecutive span of memory.
    ///
    /// # Performance
    ///
    /// Be aware that this operation can be expensive if the sequence consists of multiple spans,
    /// as it will require copying all the data into a new `Bytes` instance.
    ///
    /// # Panics
    ///
    /// TODO: Document panics
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        if self.spans.is_empty() {
            Bytes::new()
        } else if self.spans.len() == 1 {
            // We are a single-span sequence, which can potentially be zero-copy represented.
            // We delegate the choice to the span itself.
            self.spans
                .front()
                .expect("we verified there is one span")
                .to_bytes()
        } else {
            // We must copy, as Bytes can only represent consecutive spans of data.
            let mut bytes = BytesMut::with_capacity(self.remaining());

            for span in &self.spans {
                bytes.extend_from_slice(span);
            }

            debug_assert_eq!(self.remaining(), bytes.len());

            bytes.freeze()
        }
    }

    /// Fills an array of slices with chunks from the start of the sequence.
    ///
    /// This is equivalent to `Buf::chunks_vectored` but as regular slices, without the `IoSlice`
    /// wrapper, which can be unnecessary/limiting and is not always desirable.
    ///
    /// # Panics
    ///
    /// TODO: Document panics
    pub fn chunks_as_slices_vectored<'a>(&'a self, dst: &mut [&'a [u8]]) -> usize {
        if dst.is_empty() {
            return 0;
        }

        // How many chunks can we fill?
        let chunk_count = self.spans.len().min(dst.len());

        for (i, span) in self.spans.iter().take(chunk_count).enumerate() {
            *dst.get_mut(i).expect("guarded by min()") = span;
        }

        chunk_count
    }
}

impl Default for Sequence {
    fn default() -> Self {
        Self::empty()
    }
}

impl Buf for Sequence {
    fn remaining(&self) -> usize {
        // TODO: Cache this value so we do not have to recalculate it every time.
        self.spans.iter().map(bytes::Buf::remaining).sum()
    }

    #[cfg_attr(test, mutants::skip)] // Mutating this can cause infinite loops.
    fn chunk(&self) -> &[u8] {
        if self.spans.is_empty() {
            return &[];
        }

        self.spans.front().expect("already handled empty case")
    }

    fn chunks_vectored<'a>(&'a self, dst: &mut [IoSlice<'a>]) -> usize {
        if dst.is_empty() {
            return 0;
        }

        // How many chunks can we fill?
        let chunk_count = self.spans.len().min(dst.len());

        // Note that IoSlice has a length limit of u32::MAX. Our spans are also limited to u32::MAX
        // by memory manager internal limits (MAX_BLOCK_SIZE), so this is safe.
        for (i, span) in self.spans.iter().take(chunk_count).enumerate() {
            *dst.get_mut(i).expect("guarded by min()") = IoSlice::new(span);
        }

        chunk_count
    }

    #[cfg_attr(test, mutants::skip)] // Mutating this can cause infinite loops.
    fn advance(&mut self, mut cnt: usize) {
        assert!(cnt <= self.remaining());

        while cnt > 0 {
            let front = self
                .spans
                .front_mut()
                .expect("logic error - ran out of spans before advancing over their contents");
            let remaining = front.remaining();

            if cnt < remaining {
                front.advance(cnt);
                break;
            }

            self.spans.pop_front();
            cnt = cnt
                .checked_sub(remaining)
                .expect("already handled cnt < remaining case");
        }
    }
}

impl From<Sequence> for Bytes {
    fn from(sequence: Sequence) -> Self {
        sequence.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        clippy::needless_range_loop,
        clippy::arithmetic_side_effects,
        reason = "This is all fine in test code"
    )]

    use std::num::NonZero;
    use std::sync::Arc;
    use std::thread;

    use static_assertions::assert_impl_all;

    use super::*;
    use crate::testing::assert_panic;
    use crate::{Block, FakeMemoryProvider, SequenceBuilder};

    #[test]
    fn smoke_test() {
        let block = Block::new(NonZero::new(10).unwrap());

        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let mut builder = unsafe { block.take_ownership() };

        builder.put_u64(1234);
        builder.put_u16(16);

        let span1 = builder.consume(NonZero::new(4).unwrap());
        let span2 = builder.consume(NonZero::new(3).unwrap());
        let span3 = builder.consume(NonZero::new(3).unwrap());

        assert_eq!(0, builder.remaining_mut());
        assert_eq!(span1.remaining(), 4);
        assert_eq!(span2.remaining(), 3);
        assert_eq!(span3.remaining(), 3);

        let mut sequence = Sequence::from_spans(vec![span1, span2, span3]);

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
        let block = Block::new(NonZero::new(10).unwrap());

        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let mut builder = unsafe { block.take_ownership() };

        builder.put_u64(1234);
        builder.put_u16(16);

        let span1 = builder.consume(NonZero::new(4).unwrap());
        let span2 = builder.consume(NonZero::new(3).unwrap());
        let span3 = builder.consume(NonZero::new(3).unwrap());

        let mut sequence = Sequence::from_spans(vec![span1, span2, span3]);

        assert_eq!(10, sequence.remaining());

        assert_eq!(sequence.get_u64(), 1234);
        assert_panic!(_ = sequence.get_u32()); // Reads 4 but only has 2 remaining.
    }

    #[test]
    fn extend_lifetime_references_all_blocks() {
        let mut weak_references = Vec::new();

        let guard = {
            let block1 = Block::new(NonZero::new(8).unwrap());
            weak_references.push(Arc::downgrade(&block1));

            // SAFETY: We declare exclusivity.
            let mut builder1 = unsafe { block1.take_ownership() };
            builder1.put_u64(1111);
            let span1 = builder1.consume(NonZero::new(8).unwrap());

            let block2 = Block::new(NonZero::new(10).unwrap());
            weak_references.push(Arc::downgrade(&block2));

            // SAFETY: We declare exclusivity.
            let mut builder2 = unsafe { block2.take_ownership() };
            builder2.put_u64(2222);
            let span2 = builder2.consume(NonZero::new(8).unwrap());

            let sequence = Sequence::from_spans(vec![span1, span2]);

            sequence.extend_lifetime()
        };

        // The guard should keep both weakly referenced blocks alive.
        assert!(weak_references.iter().all(|weak| weak.upgrade().is_some()));

        drop(guard);

        // And now they should all be dead.
        assert!(weak_references.iter().all(|weak| weak.upgrade().is_none()));
    }

    #[test]
    fn from_sequences() {
        let block = Block::new(NonZero::new(100).unwrap());

        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let mut builder = unsafe { block.take_ownership() };

        builder.put_u64(1234);
        builder.put_u64(5678);

        let span1 = builder.consume(NonZero::new(8).unwrap());
        let span2 = builder.consume(NonZero::new(8).unwrap());

        let sequence1 = Sequence::from_spans(vec![span1]);
        let sequence2 = Sequence::from_spans(vec![span2]);

        let mut combined = Sequence::from_sequences(vec![sequence1, sequence2]);

        assert_eq!(16, combined.remaining());

        assert_eq!(combined.get_u64(), 1234);
        assert_eq!(combined.get_u64(), 5678);
    }

    #[test]
    fn empty_sequence() {
        let sequence = Sequence::default();

        assert!(sequence.is_empty());
        assert_eq!(0, sequence.remaining());
        assert_eq!(0, sequence.chunk().len());

        let bytes = sequence.into_bytes();
        assert_eq!(0, bytes.len());
    }

    #[test]
    fn into_bytes() {
        let block = Block::new(NonZero::new(100).unwrap());

        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let mut builder = unsafe { block.take_ownership() };

        builder.put_u64(1234);
        builder.put_u64(5678);

        let span1 = builder.consume(NonZero::new(8).unwrap());
        let span2 = builder.consume(NonZero::new(8).unwrap());

        let sequence_single_span = Sequence::from_spans(vec![span1.clone()]);
        let sequence_multi_span = Sequence::from_spans(vec![span1, span2]);

        let mut bytes = sequence_single_span.clone().into_bytes();
        assert_eq!(8, bytes.len());
        assert_eq!(1234, bytes.get_u64());

        let mut bytes = Bytes::from(sequence_single_span);
        assert_eq!(8, bytes.len());
        assert_eq!(1234, bytes.get_u64());

        let mut bytes = sequence_multi_span.into_bytes();
        assert_eq!(16, bytes.len());
        assert_eq!(1234, bytes.get_u64());
        assert_eq!(5678, bytes.get_u64());
    }

    #[test]
    fn thread_safe_type() {
        assert_impl_all!(Sequence: Send, Sync);
    }

    #[test]
    fn slice_from_single_span_sequence() {
        const SPAN_SIZE: NonZero<usize> = NonZero::new(100).unwrap();

        // A very simple sequence to start with, consisting of just one 100 byte span.
        let block = Block::new(SPAN_SIZE);
        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let span_builder = unsafe { block.take_ownership() };

        let mut sb = SequenceBuilder::from_span_builders([span_builder]);

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
        const SPAN_SIZE: NonZero<usize> = NonZero::new(10).unwrap();

        // A multi-span sequence, 10 bytes x10.
        let span_builders = (0..10).map(|_| {
            let block = Block::new(SPAN_SIZE);
            // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
            unsafe { block.take_ownership() }
        });

        let mut sb = SequenceBuilder::from_span_builders(span_builders);

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
        let block = Block::new(NonZero::new(10).unwrap());
        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let span_builder = unsafe { block.take_ownership() };

        let mut sb = SequenceBuilder::from_span_builders([span_builder]);
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
    fn slice_oob_is_panic() {
        let block = Block::new(NonZero::new(100).unwrap());
        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let span_builder = unsafe { block.take_ownership() };

        let mut sb = SequenceBuilder::from_span_builders([span_builder]);
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
        let block = Block::new(NonZero::new(100).unwrap());
        // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
        let span_builder = unsafe { block.take_ownership() };

        let mut sb = SequenceBuilder::from_span_builders([span_builder]);
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
    fn consume_all_chunks() {
        const SPAN_SIZE: NonZero<usize> = NonZero::new(10).unwrap();

        // A multi-span sequence, 10 bytes x10.
        let span_builders = (0..10).map(|_| {
            let block = Block::new(SPAN_SIZE);
            // SAFETY: We must guarantee exclusive ownership. I hereby promise exclusivity!
            unsafe { block.take_ownership() }
        });

        let mut sb = SequenceBuilder::from_span_builders(span_builders);

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
        fn post_to_another_thread(s: Sequence) {
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

        let s = FakeMemoryProvider::copy_from_static(b"Hello, world!");

        post_to_another_thread(s);
    }

    #[test]
    fn vectored_read_as_io_slice() {
        let segment1 = FakeMemoryProvider::copy_from_static(b"Hello, world!");
        let segment2 = FakeMemoryProvider::copy_from_static(b"Hello, another world!");

        let sequence = Sequence::from_sequences(vec![segment1.clone(), segment2.clone()]);

        let mut io_slices = vec![IoSlice::new(&[]); 4];
        let ioslice_count = sequence.chunks_vectored(&mut io_slices);

        assert_eq!(ioslice_count, 2);
        assert_eq!(io_slices[0].len(), segment1.len());
        assert_eq!(io_slices[1].len(), segment2.len());
    }

    #[test]
    fn vectored_read_as_slice() {
        let segment1 = FakeMemoryProvider::copy_from_static(b"Hello, world!");
        let segment2 = FakeMemoryProvider::copy_from_static(b"Hello, another world!");

        let sequence = Sequence::from_sequences(vec![segment1.clone(), segment2.clone()]);

        let mut slices: Vec<&[u8]> = vec![&[]; 4];
        let slice_count = sequence.chunks_as_slices_vectored(&mut slices);

        assert_eq!(slice_count, 2);
        assert_eq!(slices[0].len(), segment1.len());
        assert_eq!(slices[1].len(), segment2.len());
    }

    #[test]
    fn from_bytes_ok() {
        let bytes = Bytes::from_static(b"Hello, world!");
        let sequence = Sequence::from_bytes(bytes, &FakeMemoryProvider);

        assert_eq!(sequence.remaining(), 13);
    }
}