// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::panic;
use std::mem::{self, MaybeUninit};
use std::num::NonZero;

use bytes::buf::UninitSlice;
use bytes::{Buf, BufMut};
use smallvec::SmallVec;

use crate::{Block, BlockSize, BytesBufWrite, BytesView, MAX_INLINE_SPANS, Memory, MemoryGuard, Span, SpanBuilder};

/// Owns some memory capacity in which it allows you to place a sequence of bytes that
/// you can thereafter extract as one or more [`BytesView`]s.
///
/// The capacity of the `BytesBuf` must be reserved in advance via [`reserve()`][3] before
/// you can fill it with data.
///
/// # Memory capacity
///
/// A single `BytesBuf` can use memory capacity from any [memory provider][7], including a
/// mix of different memory providers for the same `BytesBuf` instance. All methods that
/// extend the memory capacity require the caller to provide a reference to the memory provider.
///
/// # Conceptual design
///
/// The memory owned by a `BytesBuf` (its capacity) can be viewed as two regions:
///
/// * Filled memory - these bytes have been written to but have not yet been consumed as a
///   [`BytesView`]. They may be peeked at (via [`peek()`][4]) or consumed (via [`consume()`][5]).
/// * Available memory - these bytes have not yet been written to and are available for writing via
///   [`bytes::buf::BufMut`][1] or [`begin_vectored_write()`][Self::begin_vectored_write].
///
/// Existing [`BytesView`]s can be appended to the [`BytesBuf`] via [`append()`][6] without
/// consuming capacity (each appended [`BytesView`] brings its own backing memory capacity).
#[doc = include_str!("../doc/snippets/sequence_memory_layout.md")]
///
/// [1]: https://docs.rs/bytes/latest/bytes/buf/trait.BufMut.html
/// [3]: Self::reserve
/// [4]: Self::peek
/// [5]: Self::consume
/// [6]: Self::append
/// [7]: crate::Memory
#[derive(Default)]
pub struct BytesBuf {
    // The frozen spans are at the front of the sequence being built and have already become
    // immutable (or already arrived in that form). They will be consumed first.
    //
    // Optimization: we might get slightly better performance by using a stack-preferring queue
    // here instead. No suitable crate was found at time of writing, may need to invent it.
    frozen_spans: SmallVec<[Span; MAX_INLINE_SPANS]>,

    // The span builders contain "potential spans" that have not yet been materialized/frozen.
    // The first item may be partially filled with data, with the others being spare capacity.
    //
    // Exception: a vectored write may write to any number of span builders concurrently but when
    // the vectored write is committed we immediately restore the above situation (with only
    // the first span builder potentially containing data).
    //
    // When the capacity of a span builder is exhausted, we transform any data in it into a span
    // and move it to `frozen_spans`.
    //
    // Partially filled span builders may be split into a span and a builder over the remaining
    // memory. This happens on demand when the sequence builder needs to emit data from part of
    // a span builder's memory region.
    //
    // Note that we do not require the span builders to be of the same capacity.
    //
    // We store the span builders in reverse order - the logically first span builder (which we
    // may have partially filled with content) is the last one in this collection.
    //
    // Optimization: we might get slightly better performance by using a stack-preferring queue
    // here instead. No suitable crate was found at time of writing, may need to invent it.
    span_builders_reversed: SmallVec<[SpanBuilder; MAX_INLINE_SPANS]>,

    /// Length of the filled memory in this sequence builder.
    ///
    /// We cache this to avoid recalculating it every time we need this information.
    len: usize,

    /// Length of the data contained in the frozen spans. The total `len` is this plus whatever
    /// may be partially filled in the (logically) first span builder.
    ///
    /// We cache this to avoid recalculating it every time we need this information.
    frozen: usize,

    /// Available capacity that can accept additional data into it.
    /// The total capacity is `len` + `available`.
    ///
    /// We cache this to avoid recalculating it every time we need this information.
    available: usize,
}

impl BytesBuf {
    /// Creates an instance with 0 bytes of capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an instance that takes exclusive ownership of the capacity in the
    /// provided memory blocks.
    ///
    /// This is used by implementations of memory providers. To obtain a `BytesBuf` with
    /// available memory capacity, you need to use an implementation of [`Memory`] that
    /// provides you instances of `BytesBuf`.
    ///
    /// There is no guarantee that the `BytesBuf` uses the blocks in the order provided to
    /// this function. Blocks may be used in any order.
    pub fn from_blocks<I>(blocks: I) -> Self
    where
        I: IntoIterator<Item = Block>,
    {
        Self::from_span_builders(blocks.into_iter().map(Block::into_span_builder))
    }

    pub(crate) fn from_span_builders<I>(span_builders: I) -> Self
    where
        I: IntoIterator<Item = SpanBuilder>,
    {
        let span_builders: SmallVec<[SpanBuilder; MAX_INLINE_SPANS]> = span_builders.into_iter().collect();

        let available = span_builders.iter().map(SpanBuilder::remaining_mut).sum();

        Self {
            frozen_spans: SmallVec::new_const(),
            // We do not expect the order that we use the span builders to matter,
            // so we do not reverse them here before storing.
            span_builders_reversed: span_builders,
            len: 0,
            frozen: 0,
            available,
        }
    }

    /// Adds memory capacity to the sequence builder, ensuring there is enough capacity to
    /// accommodate `additional_bytes` of content in addition to existing content already present.
    ///
    /// The requested reserve capacity may be extended further if the memory provider considers it
    /// more efficient to use a larger block of memory than strictly required for this operation.
    pub fn reserve(&mut self, additional_bytes: usize, memory_provider: &impl Memory) {
        let bytes_needed = additional_bytes.saturating_sub(self.remaining_mut());

        if bytes_needed == 0 {
            return;
        }

        self.extend_capacity_by_at_least(bytes_needed, memory_provider);
    }

    fn extend_capacity_by_at_least(&mut self, bytes: usize, memory_provider: &impl Memory) {
        let additional_memory = memory_provider.reserve(bytes);

        // For extra paranoia. We expect a memory provider to return an empty sequence builder.
        debug_assert!(additional_memory.capacity() >= bytes);
        debug_assert!(additional_memory.is_empty());

        self.available = self
            .available
            .checked_add(additional_memory.capacity())
            .expect("usize overflow should be impossible here because the sequence builder capacity would exceed virtual memory size");

        // We put the new ones in front (existing content needs to stay at the end).
        self.span_builders_reversed.insert_many(0, additional_memory.span_builders_reversed);
    }

    /// Appends the given sequence to the end of the sequence builder's filled bytes region.
    ///
    /// This automatically extends the builder's capacity with the memory capacity used of the
    /// appended sequence, for a net zero change in remaining available capacity.
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn append(&mut self, sequence: BytesView) {
        if !sequence.has_remaining() {
            return;
        }

        let sequence_len = sequence.len();

        // Only the first span builder may hold unfrozen data (the rest are for spare capacity).
        let total_unfrozen_bytes = NonZero::new(self.span_builders_reversed.last().map_or(0, SpanBuilder::len));

        if let Some(total_unfrozen_bytes) = total_unfrozen_bytes {
            // If there is any unfrozen data, we freeze it now to ensure we append after all
            // existing data already in the sequence builder.
            self.freeze_from_first(total_unfrozen_bytes);

            // Debug build paranoia: nothing remains in the span builder, right?
            debug_assert!(self.span_builders_reversed.last().map_or(0, SpanBuilder::len) == 0);
        }

        self.frozen_spans.extend(sequence.into_spans_reversed().into_iter().rev());

        self.len = self
            .len
            .checked_add(sequence_len)
            .expect("usize overflow should be impossible here because the sequence builder capacity would exceed virtual memory size");

        // Any appended BytesView is frozen by definition, as contents of a BytesView are immutable.
        self.frozen = self
            .frozen
            .checked_add(sequence_len)
            .expect("usize overflow should be impossible here because the sequence builder capacity would exceed virtual memory size");
    }

    /// Peeks at the contents of the filled bytes region, returning a [`BytesView`] over all
    /// filled data without consuming it from the sequence builder.
    ///
    /// This is similar to [`consume_all()`][Self::consume_all] except the data remains in the
    /// sequence builder and can still be consumed later. The capacity of any partially filled
    /// span builder is preserved.
    ///
    /// # Performance
    ///
    /// This operation freezes any unfrozen data in the sequence builder, which is a relatively
    /// cheap operation but does involve creating a new span. Subsequent calls to `peek()` will
    /// be very cheap if no new data has been added.
    #[must_use]
    pub fn peek(&self) -> BytesView {
        // Build a list of all spans to include in the result, in reverse order for efficient construction.
        let mut result_spans_reversed: SmallVec<[Span; MAX_INLINE_SPANS]> = SmallVec::new();

        // If there is unfrozen data in the first span builder, we need to create a span for it.
        // We do NOT consume it from the builder - we just create a new span that references the same memory.
        if let Some(first_builder) = self.span_builders_reversed.last() {
            if let Some(span) = first_builder.peek_filled() {
                result_spans_reversed.push(span);
            }
        }

        // Add all the frozen spans (they are already in content order in our storage).
        result_spans_reversed.extend(self.frozen_spans.iter().rev().cloned());

        BytesView::from_spans_reversed(result_spans_reversed)
    }

    /// Length of the filled bytes region, ready to be consumed.
    #[must_use]
    #[cfg_attr(debug_assertions, expect(clippy::missing_panics_doc, reason = "only unreachable panics"))]
    pub fn len(&self) -> usize {
        #[cfg(debug_assertions)]
        assert_eq!(self.len, self.calculate_len());

        self.len
    }

    #[cfg(debug_assertions)]
    fn calculate_len(&self) -> usize {
        let frozen_len = self.frozen_spans.iter().map(|x| x.len() as usize).sum::<usize>();
        let unfrozen_len = self.span_builders_reversed.last().map_or(0, SpanBuilder::len) as usize;

        frozen_len
            .checked_add(unfrozen_len)
            .expect("usize overflow should be impossible here because the sequence builder would exceed virtual memory size")
    }

    /// Whether the filled bytes region is empty, i.e. contains no bytes that can be consumed.
    ///
    /// This does not imply that the sequence builder has no remaining capacity.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The total capacity of the sequence builder.
    ///
    /// This is the sum of the length of the filled bytes and the available bytes regions.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn capacity(&self) -> usize {
        self.len()
            .checked_add(self.remaining_mut())
            .expect("usize overflow should be impossible here because the sequence builder would exceed virtual memory size")
    }

    /// How many more bytes can be written into the sequence builder before its memory capacity
    /// is exhausted.
    #[cfg_attr(test, mutants::skip)] // Lying about length is an easy way to infinite loops.
    pub fn remaining_mut(&self) -> usize {
        // The remaining capacity is the sum of the remaining capacity of all span builders.
        debug_assert_eq!(
            self.available,
            self.span_builders_reversed.iter().map(bytes::BufMut::remaining_mut).sum::<usize>()
        );

        self.available
    }

    /// Consumes `len` bytes from the beginning of the filled bytes region,
    /// returning a [`BytesView`] with those bytes.
    ///
    /// # Panics
    ///
    /// Panics if the filled bytes region does not contain at least `len` bytes.
    pub fn consume(&mut self, len: usize) -> BytesView {
        self.consume_checked(len)
            .expect("attempted to consume more bytes than available in builder")
    }

    /// Consumes `len` bytes from the beginning of the filled bytes region,
    /// returning a [`BytesView`] with those bytes.
    ///
    /// Returns `None` if the filled bytes region does not contain at least `len` bytes.
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn consume_checked(&mut self, len: usize) -> Option<BytesView> {
        if len > self.len() {
            return None;
        }

        self.ensure_frozen(len);

        let manifest = self.prepare_consume(len);

        // We build the result spans collection up in storage order.
        // The first piece of content goes last into the result spans.
        let mut result_spans_reversed: SmallVec<[Span; MAX_INLINE_SPANS]> = SmallVec::with_capacity(manifest.required_spans_capacity());

        // The content-order last span goes first into the result, so if we have a partial
        // span, shove it in there first. The fully detached spans get processed together.
        if manifest.consume_partial_span_bytes != 0 {
            // We also need some bytes from the first frozen span that now remains
            // but not the entire frozen span.
            let partially_consumed_frozen_span = self
                .frozen_spans
                .get_mut(manifest.detach_complete_frozen_spans)
                .expect("guarded by ensure_frozen()");

            let take = partially_consumed_frozen_span.slice(0..manifest.consume_partial_span_bytes);
            result_spans_reversed.push(take);

            partially_consumed_frozen_span.advance(manifest.consume_partial_span_bytes as usize);
        }

        // We extend the result spans with the (storage-order) fully detached spans.
        // BytesBuf stores the frozen spans in content order, so we must reverse.
        result_spans_reversed.extend(self.frozen_spans.drain(..manifest.detach_complete_frozen_spans).rev());

        self.len = self.len.checked_sub(len).expect("guarded by if-block above");

        self.frozen = self.frozen.checked_sub(len).expect("any data consumed must have first been frozen");

        Some(BytesView::from_spans_reversed(result_spans_reversed))
    }

    fn prepare_consume(&self, mut len: usize) -> ConsumeManifest {
        debug_assert!(len <= self.frozen);

        let mut detach_complete_frozen_spans: usize = 0;

        for span in &self.frozen_spans {
            let span_len = span.len();

            if span_len as usize <= len {
                detach_complete_frozen_spans = detach_complete_frozen_spans
                    .checked_add(1)
                    .expect("span count can never exceed virtual memory size");

                len = len
                    .checked_sub(span_len as usize)
                    .expect("somehow ended up with negative bytes remaining - algorithm defect");

                if len != 0 {
                    // We will consume this whole span and need more - go to next one.
                    continue;
                }
            }

            // This span satisfied our needs, either in full or in part.
            break;
        }

        ConsumeManifest {
            detach_complete_frozen_spans,
            // If any `len` was left, it was not a full span.
            consume_partial_span_bytes: len.try_into().expect("we are supposed to have less than one span of data remaining but its length does not fit into a single memory block - algorithm defect"),
        }
    }

    /// Consumes all filled bytes (if any), returning a [`BytesView`] with those bytes.
    pub fn consume_all(&mut self) -> BytesView {
        self.consume_checked(self.len()).unwrap_or_default()
    }

    /// Consumes `len` bytes from the first span builder and moves it to the frozen spans list.
    fn freeze_from_first(&mut self, len: NonZero<BlockSize>) {
        let span_builder = self
            .span_builders_reversed
            .last_mut()
            .expect("there must be at least one span builder for it to be possible to freeze bytes");

        debug_assert!(len.get() <= span_builder.len());

        let span = span_builder.consume(len);
        self.frozen_spans.push(span);

        if span_builder.remaining_mut() == 0 {
            // No more capacity left in this builder, so drop it.
            self.span_builders_reversed.pop();
        }

        self.frozen = self
            .frozen
            .checked_add(len.get() as usize)
            .expect("usize overflow should be impossible here because the sequence builder capacity would exceed virtual memory size");
    }

    /// Ensures that the frozen spans list contains at least `len` bytes of data, freezing
    /// additional data from the span builders if necessary.
    ///
    /// # Panics
    ///
    /// Panics if there is not enough data in the span builders to fulfill the request.
    fn ensure_frozen(&mut self, len: usize) {
        let must_freeze_bytes: BlockSize = len
            .saturating_sub(self.frozen)
            .try_into()
            .expect("requested to freeze more bytes from the first block than can actually fit into one block");

        let Some(must_freeze_bytes) = NonZero::new(must_freeze_bytes) else {
            return;
        };

        // We only need to freeze from the first span builder because a type invariant is that
        // only the first span builder may contain data. The others are just spare capacity.
        self.freeze_from_first(must_freeze_bytes);
    }

    /// The first consecutive slice of memory that makes up the remaining
    /// capacity of the sequence builder.
    ///
    /// After writing data to the start of this chunk, call `advance_mut()` to indicate
    /// how many bytes have been filled with data. The next call to `chunk_mut()` will
    /// return the next consecutive slice of memory you can fill.
    pub fn chunk_mut(&mut self) -> &mut bytes::buf::UninitSlice {
        // We are required to always return something, even if we have no span builders!
        self.span_builders_reversed
            .last_mut()
            .map_or_else(|| UninitSlice::uninit(&mut []), |x| x.chunk_mut())
    }

    /// Advances the write head by `count` bytes, indicating that this many bytes from the start
    /// of [`chunk_mut()`][1] have been filled with data.
    ///
    /// After this call, the indicated number of additional bytes may be consumed from the builder.
    ///
    /// # Panics
    ///
    /// Panics if `count` is greater than the length of [`chunk_mut()`][1].
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the indicated number of bytes have been initialized with
    /// data, sequentially starting from the beginning of the chunk returned by [`chunk_mut()`][1].
    ///
    /// [1]: Self::chunk_mut
    pub unsafe fn advance_mut(&mut self, count: usize) {
        if count == 0 {
            return;
        }

        // Advancing the writer by more than a single chunk's length is an error, at least under
        // the current implementation that does not support vectored BufMut access.
        assert!(
            count
                <= self
                    .span_builders_reversed
                    .last()
                    .expect("attempted to BufMut::advance_mut() when out of memory capacity - API contract violation")
                    .remaining_mut()
        );

        let span_builder = self
            .span_builders_reversed
            .last_mut()
            .expect("there must be at least one span builder if we wrote nonzero bytes");

        // SAFETY: We simply rely on the caller's safety promises here, "forwarding" them.
        unsafe { span_builder.advance_mut(count) };

        if span_builder.remaining_mut() == 0 {
            // The span builder is full, so we need to freeze it and move it to the frozen spans.
            let len = NonZero::new(span_builder.len())
                .expect("there is no capacity left in the span builder so there must be at least one byte to consume unless we somehow left an empty span builder in the queue");

            self.freeze_from_first(len);

            // Debug build paranoia: no full span remains after freeze, right?
            debug_assert!(self.span_builders_reversed.last().map_or(usize::MAX, BufMut::remaining_mut) > 0);
        }

        self.len = self
            .len
            .checked_add(count)
            .expect("usize overflow should be impossible here because the sequence builder capacity would exceed virtual memory size");

        self.available = self
            .available
            .checked_sub(count)
            .expect("guarded by assertion above - we must have at least this much capacity still available");
    }

    /// Begins a vectored write operation that takes exclusive ownership of the sequence builder
    /// for the duration of the operation and allows individual slices of available capacity to be
    /// filled concurrently, up to an optional limit of `max_len` bytes.
    ///
    /// Some I/O operations are naturally limited to a maximum number of bytes that can be
    /// transferred, so the length limit here allows us to project a restricted view of the
    /// available capacity to the caller without having to limit the true capacity of the builder.
    ///
    /// # Panics
    ///
    /// Panics if `max_len` is greater than the remaining capacity of the sequence builder.
    pub fn begin_vectored_write(&mut self, max_len: Option<usize>) -> BytesBufVectoredWrite<'_> {
        self.begin_vectored_write_checked(max_len)
            .expect("attempted to begin a vectored write with a max_len that was greater than the remaining capacity")
    }

    /// Begins a vectored write operation that takes exclusive ownership of the sequence builder
    /// for the duration of the operation and allows individual slices of available capacity to be
    /// filled concurrently, up to an optional limit of `max_len` bytes.
    ///
    /// Some I/O operations are naturally limited to a maximum number of bytes that can be
    /// transferred, so the length limit here allows us to project a restricted view of the
    /// available capacity to the caller without having to limit the true capacity of the builder.
    ///
    /// # Returns
    ///
    /// Returns `None` if `max_len` is greater than the remaining capacity of the sequence builder.
    pub fn begin_vectored_write_checked(&mut self, max_len: Option<usize>) -> Option<BytesBufVectoredWrite<'_>> {
        if let Some(max_len) = max_len
            && max_len > self.remaining_mut()
        {
            return None;
        }

        Some(BytesBufVectoredWrite { builder: self, max_len })
    }

    fn iter_available_capacity(&mut self, max_len: Option<usize>) -> BytesBufAvailableIterator<'_> {
        let next_span_builder_index = if self.span_builders_reversed.is_empty() { None } else { Some(0) };

        BytesBufAvailableIterator {
            builder: self,
            next_span_builder_index,
            max_len,
        }
    }

    /// Creates a memory guard that extends the lifetime of the memory blocks that provide the
    /// backing memory capacity for this sequence builder.
    ///
    /// This can be useful when unsafe code is used to reference the contents of a `BytesBuf`
    /// and it is possible to reach a condition where the `BytesBuf` itself no longer exists,
    /// even though the contents are referenced (e.g. because this is happening in non-Rust code).
    pub fn extend_lifetime(&self) -> MemoryGuard {
        MemoryGuard::new(
            self.span_builders_reversed
                .iter()
                .map(SpanBuilder::block)
                .map(Clone::clone)
                .chain(self.frozen_spans.iter().map(Span::block_ref).map(Clone::clone)),
        )
    }

    /// Exposes the instance through the [`Write`][std::io::Write] trait.
    ///
    /// The memory capacity of the `BytesBuf` will be automatically extended on demand
    /// with additional capacity from the supplied memory provider.
    #[inline]
    pub fn as_write<M: Memory>(&mut self, memory: &M) -> impl std::io::Write {
        BytesBufWrite::new(self, memory)
    }
}

// SAFETY: The trait documentation does not define any safety requirements we need to fulfill.
// It is unclear why the trait is marked unsafe in the first place.
unsafe impl BufMut for BytesBuf {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn remaining_mut(&self) -> usize {
        self.remaining_mut()
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    unsafe fn advance_mut(&mut self, cnt: usize) {
        // SAFETY: Forwarding safety requirements to the caller.
        unsafe {
            self.advance_mut(cnt);
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    #[inline]
    fn chunk_mut(&mut self) -> &mut bytes::buf::UninitSlice {
        self.chunk_mut()
    }
}

impl std::fmt::Debug for BytesBuf {
    #[cfg_attr(test, mutants::skip)] // We have no API contract here.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let frozen_spans = self.frozen_spans.iter().map(|x| x.len().to_string()).collect::<Vec<_>>().join(", ");

        let span_builders = self
            .span_builders_reversed
            .iter()
            .rev()
            .map(|x| {
                if x.is_empty() {
                    x.remaining_mut().to_string()
                } else {
                    format!("{} + {}", x.len(), x.remaining_mut())
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        f.debug_struct("BytesBuf")
            .field("len", &self.len)
            .field("frozen", &self.frozen)
            .field("available", &self.available)
            .field("frozen_spans", &frozen_spans)
            .field("span_builders", &span_builders)
            .finish()
    }
}

/// A prepared "consume bytes" operation, identifying what must be done to perform the operation.
#[derive(Debug, Clone, Copy)]
struct ConsumeManifest {
    /// How many frozen spans are to be fully detached from the front of the collection.
    detach_complete_frozen_spans: usize,

    /// How many bytes of data to consume from the first remaining frozen span.
    /// Any remainder is left within that span - the span itself is not detached.
    consume_partial_span_bytes: BlockSize,
}

impl ConsumeManifest {
    const fn required_spans_capacity(&self) -> usize {
        if self.consume_partial_span_bytes != 0 {
            self.detach_complete_frozen_spans
                .checked_add(1)
                .expect("span count cannot exceed virtual memory size")
        } else {
            self.detach_complete_frozen_spans
        }
    }
}

/// A vectored write is an operation that concurrently writes data into multiple chunks
/// of memory owned by a `BytesBuf`.
///
/// The operation takes exclusive ownership of the `BytesBuf`. During the vectored write,
/// the remaining capacity of the `BytesBuf` is exposed as `MaybeUninit<u8>` slices
/// that at the end of the operation must be filled sequentially and in order, without gaps,
/// in any desired amount (from 0 bytes written to all slices filled).
///
/// The capacity used during the operation can optionally be limited to `max_len` bytes.
///
/// The operation is completed by calling `.commit()` on the instance, after which the instance is
/// consumed and the exclusive ownership of the `BytesBuf` released.
///
/// If the type is dropped without committing, the operation is aborted and all remaining capacity
/// is left in a potentially uninitialized state.
#[derive(Debug)]
pub struct BytesBufVectoredWrite<'a> {
    builder: &'a mut BytesBuf,
    max_len: Option<usize>,
}

impl BytesBufVectoredWrite<'_> {
    /// Iterates over the chunks of available capacity in the sequence builder,
    /// allowing them to be filled with data.
    pub fn iter_chunks_mut(&mut self) -> BytesBufAvailableIterator<'_> {
        self.builder.iter_available_capacity(self.max_len)
    }

    /// Creates a memory guard that extends the lifetime of the memory blocks that provide the
    /// backing memory capacity for this sequence builder.
    ///
    /// This can be useful when unsafe code is used to reference the contents of a `BytesBuf`
    /// and it is possible to reach a condition where the `BytesBuf` itself no longer exists,
    /// even though the contents are referenced (e.g. because this is happening in non-Rust code).
    pub fn extend_lifetime(&self) -> MemoryGuard {
        self.builder.extend_lifetime()
    }

    /// Completes the vectored write operation, committing `bytes_written` bytes of data that
    /// sequentially and completely fills chunks from the start of the provided chunks.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `bytes_written` bytes of data have actually been written
    /// into the chunks of memory, sequentially from the start.
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub unsafe fn commit(self, bytes_written: usize) {
        assert!(bytes_written <= self.builder.remaining_mut());

        if let Some(max_len) = self.max_len {
            assert!(bytes_written <= max_len);
        }

        // Ordinarily, we have a type invariant that only the first span builder may contain data,
        // with the others being spare capacity. For the duration of a vectored write, this
        // invariant is suspended (because the vectored write has an exclusive reference which makes
        // the suspension of this invariant invisible to any other caller). We must now restore this
        // invariant. We do this by advancing the write head chunk by chunk, triggering the normal
        // freezing logic as we go (to avoid implementing two versions of the same logic), until we
        // have run out of written bytes to commit.

        let mut bytes_remaining = bytes_written;

        while bytes_remaining > 0 {
            let span_builder = self
                .builder
                .span_builders_reversed
                .last_mut()
                .expect("there must be at least one span builder because we still have filled capacity remaining to freeze");

            let bytes_available = span_builder.remaining_mut();
            let bytes_to_commit = bytes_available.min(bytes_remaining);

            // SAFETY: We forward the promise from our own safety requirements to guarantee that
            // the specified number of bytes really has been written.
            unsafe { self.builder.advance_mut(bytes_to_commit) };

            bytes_remaining = bytes_remaining
                .checked_sub(bytes_to_commit)
                .expect("we somehow advanced the write head more than the count of written bytes");
        }
    }
}

/// Iterates over the available capacity of a sequence builder as part of a vectored write
/// operation, returning a sequence of `MaybeUninit<u8>` slices.
#[derive(Debug)]
pub struct BytesBufAvailableIterator<'a> {
    builder: &'a mut BytesBuf,
    next_span_builder_index: Option<usize>,

    // Self-imposed constraint on how much of the available capacity is made visible through
    // this iterator. This can be useful to limit the amount of data that can be written into
    // a `BytesBuf` during a vectored write operation without having to limit the
    // actual capacity of the `BytesBuf`.
    max_len: Option<usize>,
}

impl<'a> Iterator for BytesBufAvailableIterator<'a> {
    type Item = &'a mut [MaybeUninit<u8>];

    #[cfg_attr(test, mutants::skip)] // This gets mutated into an infinite loop which is not very helpful.
    fn next(&mut self) -> Option<Self::Item> {
        let next_span_builder_index = self.next_span_builder_index?;

        self.next_span_builder_index = Some(
            next_span_builder_index
                .checked_add(1)
                .expect("usize overflow is inconceivable here"),
        );
        if self.next_span_builder_index == Some(self.builder.span_builders_reversed.len()) {
            self.next_span_builder_index = None;
        }

        // The iterator iterates through things in content order but we need to access
        // the span builders in storage order.
        let next_span_builder_index_storage_order = self
            .builder
            .span_builders_reversed
            .len()
            .checked_sub(next_span_builder_index + 1)
            .expect("usize overflow is inconceivable here");

        let span_builder = self
            .builder
            .span_builders_reversed
            .get_mut(next_span_builder_index_storage_order)
            .expect("iterator cursor referenced a span builder that does not exist");

        // SAFETY: Must treat it as uninitialized. Yeah, we are, obviously.
        // Somewhat pointless to have the callee be marked unsafe considering
        // it returns a `MaybeUninit` already but okay whatever, we'll play along.
        let uninit_slice_mut = unsafe { span_builder.chunk_mut().as_uninit_slice_mut() };

        // SAFETY: There is nothing Rust can do to promise the reference we return is valid for 'a
        // but we can make such a promise ourselves. In essence, returning the references with 'a
        // this will extend the exclusive ownership of `BytesBuf` until all returned chunk
        // references are dropped, even if the iterator itself is dropped earlier. We can do this
        // because we know that to access the chunks requires a reference to the `BytesBuf`,
        // so as long as a chunk reference exists, access via the `BytesBuf` is blocked.
        // TODO: It would be good to have a (ui) test to verify this.
        let uninit_slice_mut = unsafe { mem::transmute::<&mut [MaybeUninit<u8>], &'a mut [MaybeUninit<u8>]>(&mut *uninit_slice_mut) };

        let uninit_slice_mut = if let Some(max_len) = self.max_len {
            // Limit the visible range of the slice if we have a size limit.
            // If this results in the slice being limited to not its full size,
            // we will also terminate the iteration
            let constrained_len = uninit_slice_mut.len().min(max_len);

            let adjusted_slice = uninit_slice_mut.get_mut(..constrained_len).expect("guarded by min() above");

            self.max_len = Some(max_len.checked_sub(constrained_len).expect("guarded by min() above"));

            if self.max_len == Some(0) {
                // Even if there are more span builders, we have returned all the capacity
                // we are allowed to return, so pretend there is nothing more to return.
                self.next_span_builder_index = None;
            }

            adjusted_slice
        } else {
            uninit_slice_mut
        };

        Some(uninit_slice_mut)
    }
}

impl From<BytesView> for BytesBuf {
    fn from(value: BytesView) -> Self {
        let mut sb = Self::new();
        sb.append(value);
        sb
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "Fine in test code, we prefer panic on error")]

    use std::pin::pin;

    use new_zealand::nz;
    use static_assertions::assert_impl_all;
    use testing_aids::assert_panic;

    use super::*;
    use crate::testing::TestMemoryBlock;
    use crate::{FixedBlockTestMemory, GlobalPool};

    const U64_SIZE: usize = size_of::<u64>();
    const TWO_U64_SIZE: usize = size_of::<u64>() + size_of::<u64>();
    const THREE_U64_SIZE: usize = size_of::<u64>() + size_of::<u64>() + size_of::<u64>();

    #[test]
    fn smoke_test() {
        let memory = FixedBlockTestMemory::new(nz!(1234));

        let min_length = 1000;

        let mut builder = memory.reserve(min_length);

        assert!(builder.capacity() >= min_length);
        assert!(builder.remaining_mut() >= min_length);
        assert!(builder.is_empty());
        assert_eq!(builder.capacity(), builder.remaining_mut());
        assert_eq!(builder.len(), 0);

        builder.put_u64(1234);
        builder.put_u64(5678);
        builder.put_u64(1234);
        builder.put_u64(5678);

        assert_eq!(builder.len(), 32);
        assert!(!builder.is_empty());

        // SAFETY: Writing 0 bytes is always valid.
        unsafe {
            builder.advance_mut(0);
        }

        let mut first16 = builder.consume(TWO_U64_SIZE);
        let mut second16 = builder.consume(TWO_U64_SIZE);

        assert_eq!(first16.len(), 16);
        assert_eq!(second16.len(), 16);
        assert_eq!(builder.len(), 0);

        assert_eq!(first16.get_u64(), 1234);
        assert_eq!(first16.get_u64(), 5678);

        assert_eq!(second16.get_u64(), 1234);
        assert_eq!(second16.get_u64(), 5678);

        builder.put_u64(1111);

        assert_eq!(builder.len(), 8);

        let mut last8 = builder.consume(U64_SIZE);

        assert_eq!(last8.len(), 8);
        assert_eq!(builder.len(), 0);

        assert_eq!(last8.get_u64(), 1111);

        assert!(builder.consume_checked(1).is_none());

        assert!(builder.consume_all().is_empty());
    }

    #[test]
    fn extend() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory = FixedBlockTestMemory::new(nz!(100));

        // Have 0, desired 10, requesting 10, will get 100.
        builder.reserve(10, &memory);

        assert_eq!(builder.capacity(), 100);
        assert_eq!(builder.remaining_mut(), 100);

        // Write 10 bytes of data just to verify that it does not affect "capacity" logic.
        builder.put_u64(1234);
        builder.put_u16(5678);

        assert_eq!(builder.len(), 10);
        assert_eq!(builder.remaining_mut(), 90);
        assert_eq!(builder.capacity(), 100);

        // Have 100, desired 10+140=150, requesting 50, will get another 100 for a total of 200.
        builder.reserve(140, &memory);

        assert_eq!(builder.len(), 10);
        assert_eq!(builder.remaining_mut(), 190);
        assert_eq!(builder.capacity(), 200);

        // Have 200, desired 10+200=210, 210-200=10, will get another 100.
        builder.reserve(200, &memory);

        assert_eq!(builder.len(), 10);
        assert_eq!(builder.remaining_mut(), 290);
        assert_eq!(builder.capacity(), 300);
    }

    #[test]
    fn append() {
        let memory = FixedBlockTestMemory::new(nz!(1234));

        let min_length = 1000;

        let mut builder1 = memory.reserve(min_length);
        let mut builder2 = memory.reserve(min_length);

        // First we make a couple pieces to append.
        builder1.put_u64(1111);
        builder1.put_u64(2222);
        builder1.put_u64(3333);
        builder1.put_u64(4444);

        let to_append1 = builder1.consume(TWO_U64_SIZE);
        let to_append2 = builder1.consume(TWO_U64_SIZE);

        // Then we prefill some data to start us off.
        builder2.put_u64(5555);
        builder2.put_u64(6666);

        // Consume a little just for extra complexity.
        let _ = builder2.consume(U64_SIZE);

        // Append the pieces.
        builder2.append(to_append1);
        builder2.append(to_append2);

        // Appending an empty sequence does nothing.
        builder2.append(BytesView::default());

        // Add some custom data at the end.
        builder2.put_u64(7777);

        assert_eq!(builder2.len(), 48);

        let mut result = builder2.consume(48);

        assert_eq!(result.get_u64(), 6666);
        assert_eq!(result.get_u64(), 1111);
        assert_eq!(result.get_u64(), 2222);
        assert_eq!(result.get_u64(), 3333);
        assert_eq!(result.get_u64(), 4444);
        assert_eq!(result.get_u64(), 7777);
    }

    #[test]
    fn consume_all_mixed() {
        let mut builder = BytesBuf::new();
        let memory = FixedBlockTestMemory::new(nz!(8));

        // Reserve some capacity and add initial data.
        builder.reserve(16, &memory);
        builder.put_u64(1111);
        builder.put_u64(2222);

        // Consume some data (the 1111).
        let _ = builder.consume(8);

        // Append a sequence (the 3333).
        let mut append_builder = BytesBuf::new();
        append_builder.reserve(8, &memory);
        append_builder.put_u64(3333);
        let sequence = append_builder.consume_all();
        builder.append(sequence);

        // Add more data (the 4444).
        builder.reserve(8, &memory);
        builder.put_u64(4444);

        // Consume all data and validate we got all the pieces.
        let mut result = builder.consume_all();

        assert_eq!(result.len(), 24);
        assert_eq!(result.get_u64(), 2222);
        assert_eq!(result.get_u64(), 3333);
        assert_eq!(result.get_u64(), 4444);
    }

    #[test]
    #[expect(clippy::cognitive_complexity, reason = "test code")]
    fn inspect_basic() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory = FixedBlockTestMemory::new(nz!(10));

        // Peeking an empty builder is fine, it is just an empty BytesView in that case.
        let peeked = builder.peek();
        assert_eq!(peeked.remaining(), 0);

        builder.reserve(100, &memory);

        assert_eq!(builder.capacity(), 100);

        builder.put_u64(1111);

        // We have 0 frozen spans and 10 span builders,
        // the first of which has 8 bytes of filled content.
        let mut peeked = builder.peek();
        assert_eq!(peeked.chunk().len(), 8);
        assert_eq!(peeked.get_u64(), 1111);
        assert_eq!(peeked.remaining(), 0);

        builder.put_u64(2222);
        builder.put_u64(3333);
        builder.put_u64(4444);
        builder.put_u64(5555);
        builder.put_u64(6666);
        builder.put_u64(7777);
        builder.put_u64(8888);
        // These will cross a span boundary so we can also observe
        // crossing that boundary during peeking.
        builder.put_bytes(9, 8);

        assert_eq!(builder.len(), 72);
        assert_eq!(builder.capacity(), 100);
        assert_eq!(builder.remaining_mut(), 28);

        // We should have 7 frozen spans and 3 span builders,
        // the first of which has 2 bytes of filled content.
        let mut peeked = builder.peek();

        assert_eq!(peeked.remaining(), 72);

        // This should be the first frozen span of 10 bytes.
        assert_eq!(peeked.chunk().len(), 10);

        assert_eq!(peeked.get_u64(), 1111);
        assert_eq!(peeked.get_u64(), 2222);

        // The length of the sequence builder does not change just because we peek at its data.
        assert_eq!(builder.len(), 72);

        // We consumed 16 bytes from the peeked view, so should be looking at the remaining 4 bytes in the 2nd span.
        assert_eq!(peeked.chunk().len(), 4);

        assert_eq!(peeked.get_u64(), 3333);
        assert_eq!(peeked.get_u64(), 4444);
        assert_eq!(peeked.get_u64(), 5555);
        assert_eq!(peeked.get_u64(), 6666);
        assert_eq!(peeked.get_u64(), 7777);
        assert_eq!(peeked.get_u64(), 8888);

        for _ in 0..8 {
            assert_eq!(peeked.get_u8(), 9);
        }

        assert_eq!(peeked.remaining(), 0);

        // Reading 0 bytes is always valid.
        peeked.advance(0);

        assert_eq!(peeked.chunk().len(), 0);

        // Fill up the remaining 28 bytes of data so we have a full sequence builder.
        builder.put_bytes(88, 28);

        let mut peeked = builder.peek();
        peeked.advance(72);

        assert_eq!(peeked.remaining(), 28);

        for _ in 0..28 {
            assert_eq!(peeked.get_u8(), 88);
        }
    }

    #[test]
    fn consume_part_of_frozen_span() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory = FixedBlockTestMemory::new(nz!(10));

        builder.reserve(100, &memory);

        assert_eq!(builder.capacity(), 100);

        builder.put_u64(1111);
        // This freezes the first span of 10, as we filled it all up.
        builder.put_u64(2222);

        let mut first8 = builder.consume(U64_SIZE);
        assert_eq!(first8.get_u64(), 1111);
        assert!(first8.is_empty());

        builder.put_u64(3333);

        let mut second16 = builder.consume(16);
        assert_eq!(second16.get_u64(), 2222);
        assert_eq!(second16.get_u64(), 3333);
        assert!(second16.is_empty());
    }

    #[test]
    fn empty_builder() {
        let mut builder = BytesBuf::new();
        assert!(builder.is_empty());
        assert!(!builder.peek().has_remaining());
        assert_eq!(0, builder.chunk_mut().len());

        let consumed = builder.consume(0);
        assert!(consumed.is_empty());

        let consumed = builder.consume_all();
        assert!(consumed.is_empty());
    }

    #[test]
    fn thread_safe_type() {
        assert_impl_all!(BytesBuf: Send, Sync);
    }

    #[test]
    fn iter_available_empty_with_capacity() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory = FixedBlockTestMemory::new(nz!(100));

        // Capacity: 0 -> 1000 (10x100)
        builder.reserve(1000, &memory);

        assert_eq!(builder.capacity(), 1000);
        assert_eq!(builder.remaining_mut(), 1000);

        let iter = builder.iter_available_capacity(None);

        // Demonstrating that we can access chunks concurrently, not only one by one.
        let chunks = iter.collect::<Vec<_>>();

        assert_eq!(chunks.len(), 10);

        for chunk in chunks {
            assert_eq!(chunk.len(), 100);
        }

        // After we have dropped all chunk references, it is again legal to access the builder.
        // This is blocked by the borrow checker while chunk references still exist.
        builder.reserve(100, &memory);
    }

    #[test]
    fn iter_available_nonempty() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory = FixedBlockTestMemory::new(nz!(8));

        // Capacity: 0 -> 16 (2x8)
        builder.reserve(TWO_U64_SIZE, &memory);

        assert_eq!(builder.capacity(), 16);
        assert_eq!(builder.remaining_mut(), 16);

        // We write an u64 - this fills half the capacity and should result in
        // the first span builder being frozen and the second remaining in its entirety.
        builder.put_u64(1234);

        assert_eq!(builder.len(), 8);
        assert_eq!(builder.remaining_mut(), 8);

        let available_chunks = builder.iter_available_capacity(None).collect::<Vec<_>>();
        assert_eq!(available_chunks.len(), 1);
        assert_eq!(available_chunks[0].len(), 8);

        // We write a u32 - this fills half the remaining capacity, which results
        // in a half-filled span builder remaining in the sequence builder.
        builder.put_u32(5678);

        assert_eq!(builder.len(), 12);
        assert_eq!(builder.remaining_mut(), 4);

        let available_chunks = builder.iter_available_capacity(None).collect::<Vec<_>>();
        assert_eq!(available_chunks.len(), 1);
        assert_eq!(available_chunks[0].len(), 4);

        // We write a final u32 to use up all the capacity.
        builder.put_u32(9012);

        assert_eq!(builder.len(), 16);
        assert_eq!(builder.remaining_mut(), 0);

        assert_eq!(builder.iter_available_capacity(None).count(), 0);
    }

    #[test]
    fn iter_available_empty_no_capacity() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);
        assert_eq!(builder.iter_available_capacity(None).count(), 0);
    }

    #[test]
    fn vectored_write_zero() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory = FixedBlockTestMemory::new(nz!(8));

        // Capacity: 0 -> 16 (2x8)
        builder.reserve(TWO_U64_SIZE, &memory);

        assert_eq!(builder.capacity(), 16);
        assert_eq!(builder.remaining_mut(), 16);

        let vectored_write = builder.begin_vectored_write(None);

        // SAFETY: Yes, we really wrote 0 bytes.
        unsafe {
            vectored_write.commit(0);
        }

        assert_eq!(builder.capacity(), 16);
        assert_eq!(builder.remaining_mut(), 16);
    }

    #[test]
    fn vectored_write_one_chunk() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory = FixedBlockTestMemory::new(nz!(8));

        // Capacity: 0 -> 8 (1x8)
        builder.reserve(U64_SIZE, &memory);

        assert_eq!(builder.capacity(), 8);
        assert_eq!(builder.remaining_mut(), 8);

        let mut vectored_write = builder.begin_vectored_write(None);

        let mut chunks = vectored_write.iter_chunks_mut().collect::<Vec<_>>();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 8);

        chunks[0].put_u64(0x3333_3333_3333_3333);

        // SAFETY: Yes, we really wrote 8 bytes.
        unsafe {
            vectored_write.commit(8);
        }

        assert_eq!(builder.len(), 8);
        assert_eq!(builder.remaining_mut(), 0);
        assert_eq!(builder.capacity(), 8);

        let mut result = builder.consume(U64_SIZE);
        assert_eq!(result.get_u64(), 0x3333_3333_3333_3333);
    }

    #[test]
    fn vectored_write_multiple_chunks() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory = FixedBlockTestMemory::new(nz!(8));

        // Capacity: 0 -> 24 (3x8)
        builder.reserve(THREE_U64_SIZE, &memory);

        assert_eq!(builder.capacity(), 24);
        assert_eq!(builder.remaining_mut(), 24);

        let mut vectored_write = builder.begin_vectored_write(None);

        let mut chunks = vectored_write.iter_chunks_mut().collect::<Vec<_>>();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 8);
        assert_eq!(chunks[1].len(), 8);
        assert_eq!(chunks[2].len(), 8);

        // We fill 12 bytes, leaving middle chunk split in half between filled/available.
        chunks[0].put_u64(0x3333_3333_3333_3333);
        chunks[1].put_u32(0x4444_4444);

        // SAFETY: Yes, we really wrote 12 bytes.
        unsafe {
            vectored_write.commit(12);
        }

        assert_eq!(builder.len(), 12);
        assert_eq!(builder.remaining_mut(), 12);
        assert_eq!(builder.capacity(), 24);

        let mut vectored_write = builder.begin_vectored_write(None);

        let mut chunks = vectored_write.iter_chunks_mut().collect::<Vec<_>>();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4);
        assert_eq!(chunks[1].len(), 8);

        chunks[0].put_u32(0x5555_5555);
        chunks[1].put_u64(0x6666_6666_6666_6666);

        // SAFETY: Yes, we really wrote 12 bytes.
        unsafe {
            vectored_write.commit(12);
        }

        assert_eq!(builder.len(), 24);
        assert_eq!(builder.remaining_mut(), 0);
        assert_eq!(builder.capacity(), 24);

        let mut result = builder.consume(THREE_U64_SIZE);
        assert_eq!(result.get_u64(), 0x3333_3333_3333_3333);
        assert_eq!(result.get_u32(), 0x4444_4444);
        assert_eq!(result.get_u32(), 0x5555_5555);
        assert_eq!(result.get_u64(), 0x6666_6666_6666_6666);
    }

    #[test]
    fn vectored_write_max_len() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory = FixedBlockTestMemory::new(nz!(8));

        // Capacity: 0 -> 24 (3x8)
        builder.reserve(THREE_U64_SIZE, &memory);

        assert_eq!(builder.capacity(), 24);
        assert_eq!(builder.remaining_mut(), 24);

        // We limit to 13 bytes of visible capacity, of which we will fill 12.
        let mut vectored_write = builder.begin_vectored_write(Some(13));

        let mut chunks = vectored_write.iter_chunks_mut().collect::<Vec<_>>();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 8);
        assert_eq!(chunks[1].len(), 5);

        // We fill 12 bytes, leaving middle chunk split in half between filled/available.
        chunks[0].put_u64(0x3333_3333_3333_3333);
        chunks[1].put_u32(0x4444_4444);

        // SAFETY: Yes, we really wrote 12 bytes.
        unsafe {
            vectored_write.commit(12);
        }

        assert_eq!(builder.len(), 12);
        assert_eq!(builder.remaining_mut(), 12);
        assert_eq!(builder.capacity(), 24);

        // There are 12 remaining and we set max_limit to exactly cover those 12
        let mut vectored_write = builder.begin_vectored_write(Some(12));

        let mut chunks = vectored_write.iter_chunks_mut().collect::<Vec<_>>();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4);
        assert_eq!(chunks[1].len(), 8);

        chunks[0].put_u32(0x5555_5555);
        chunks[1].put_u64(0x6666_6666_6666_6666);

        // SAFETY: Yes, we really wrote 12 bytes.
        unsafe {
            vectored_write.commit(12);
        }

        assert_eq!(builder.len(), 24);
        assert_eq!(builder.remaining_mut(), 0);
        assert_eq!(builder.capacity(), 24);

        let mut result = builder.consume(THREE_U64_SIZE);
        assert_eq!(result.get_u64(), 0x3333_3333_3333_3333);
        assert_eq!(result.get_u32(), 0x4444_4444);
        assert_eq!(result.get_u32(), 0x5555_5555);
        assert_eq!(result.get_u64(), 0x6666_6666_6666_6666);
    }

    #[test]
    fn vectored_write_max_len_overflow() {
        let mut builder = BytesBuf::new();

        let memory = FixedBlockTestMemory::new(nz!(8));

        // Capacity: 0 -> 24 (3x8)
        builder.reserve(THREE_U64_SIZE, &memory);

        assert_eq!(builder.capacity(), 24);
        assert_eq!(builder.remaining_mut(), 24);

        // We ask for 25 bytes of capacity but there are only 24 available. Oops!
        assert_panic!(builder.begin_vectored_write(Some(25)));
    }

    #[test]
    fn vectored_write_overcommit() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory = FixedBlockTestMemory::new(nz!(8));

        // Capacity: 0 -> 16 (2x8)
        builder.reserve(TWO_U64_SIZE, &memory);

        assert_eq!(builder.capacity(), 16);
        assert_eq!(builder.remaining_mut(), 16);

        let vectored_write = builder.begin_vectored_write(None);

        assert_panic!(
            // SAFETY: Intentionally lying here to trigger a panic.
            unsafe {
                vectored_write.commit(17);
            }
        );
    }

    #[test]
    fn vectored_write_abort() {
        let mut builder = BytesBuf::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory = FixedBlockTestMemory::new(nz!(8));

        // Capacity: 0 -> 8 (1x8)
        builder.reserve(U64_SIZE, &memory);

        assert_eq!(builder.capacity(), 8);
        assert_eq!(builder.remaining_mut(), 8);

        let mut vectored_write = builder.begin_vectored_write(None);

        let mut chunks = vectored_write.iter_chunks_mut().collect::<Vec<_>>();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 8);

        chunks[0].put_u64(0x3333_3333_3333_3333);

        // Actually never mind - we drop it here.
        #[expect(clippy::drop_non_drop, reason = "Just being explicit for illustration")]
        drop(vectored_write);

        assert_eq!(builder.len(), 0);
        assert_eq!(builder.remaining_mut(), 8);
        assert_eq!(builder.capacity(), 8);
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
            let block1 = unsafe { block1.as_ref().to_block() };
            // SAFETY: We guarantee exclusive access to the memory capacity.
            let block2 = unsafe { block2.as_ref().to_block() };

            let mut builder = BytesBuf::from_blocks([block1, block2]);

            // Freezes first span of 8, retains one span builder.
            builder.put_u64(1234);

            assert_eq!(builder.frozen_spans.len(), 1);
            assert_eq!(builder.span_builders_reversed.len(), 1);

            builder.extend_lifetime()
        };

        // The sequence builder was destroyed and all BlockRefs it was holding are gone.
        // However, the lifetime guard is still alive and has a BlockRef.

        assert_eq!(block1.ref_count(), 1);
        assert_eq!(block2.ref_count(), 1);

        drop(guard);

        // And now they should all be dead.
        assert_eq!(block1.ref_count(), 0);
        assert_eq!(block2.ref_count(), 0);
    }

    #[test]
    fn extend_lifetime_during_vectored_write_references_all_blocks() {
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
            let block1 = unsafe { block1.as_ref().to_block() };
            // SAFETY: We guarantee exclusive access to the memory capacity.
            let block2 = unsafe { block2.as_ref().to_block() };

            let mut builder = BytesBuf::from_blocks([block1, block2]);

            // Freezes first span of 8, retains one span builder.
            builder.put_u64(1234);

            assert_eq!(builder.frozen_spans.len(), 1);
            assert_eq!(builder.span_builders_reversed.len(), 1);

            let vectored_write = builder.begin_vectored_write(None);

            vectored_write.extend_lifetime()
        };

        // The sequence builder was destroyed and all BlockRefs it was holding are gone.
        // However, the lifetime guard is still alive and has a BlockRef.

        assert_eq!(block1.ref_count(), 1);
        assert_eq!(block2.ref_count(), 1);

        drop(guard);

        // And now they should all be dead.
        assert_eq!(block1.ref_count(), 0);
        assert_eq!(block2.ref_count(), 0);
    }

    #[test]
    fn from_sequence() {
        let memory = GlobalPool::new();

        let s1 = BytesView::copied_from_slice(b"bla bla bla", &memory);

        let mut sb: BytesBuf = s1.clone().into();

        let s2 = sb.consume_all();

        assert_eq!(s1, s2);
    }

    #[test]
    fn consume_manifest_correctly_calculated() {
        let memory = FixedBlockTestMemory::new(nz!(10));

        let mut builder = BytesBuf::new();
        builder.reserve(100, &memory);

        // 32 bytes in 3 spans.
        builder.put_u64(1111);
        builder.put_u64(1111);
        builder.put_u64(1111);
        builder.put_u64(1111);

        // Freeze it all - a precondition to consuming is to freeze everything first.
        builder.ensure_frozen(32);

        let consume8 = builder.prepare_consume(8);

        assert_eq!(consume8.detach_complete_frozen_spans, 0);
        assert_eq!(consume8.consume_partial_span_bytes, 8);
        assert_eq!(consume8.required_spans_capacity(), 1);

        let consume10 = builder.prepare_consume(10);

        assert_eq!(consume10.detach_complete_frozen_spans, 1);
        assert_eq!(consume10.consume_partial_span_bytes, 0);
        assert_eq!(consume10.required_spans_capacity(), 1);

        let consume11 = builder.prepare_consume(11);

        assert_eq!(consume11.detach_complete_frozen_spans, 1);
        assert_eq!(consume11.consume_partial_span_bytes, 1);
        assert_eq!(consume11.required_spans_capacity(), 2);

        let consume30 = builder.prepare_consume(30);

        assert_eq!(consume30.detach_complete_frozen_spans, 3);
        assert_eq!(consume30.consume_partial_span_bytes, 0);
        assert_eq!(consume30.required_spans_capacity(), 3);

        let consume31 = builder.prepare_consume(31);

        assert_eq!(consume31.detach_complete_frozen_spans, 3);
        assert_eq!(consume31.consume_partial_span_bytes, 1);
        assert_eq!(consume31.required_spans_capacity(), 4);

        let consume32 = builder.prepare_consume(32);

        // Note that even though our memory comes in blocks of 10, there are only 2 bytes
        // in the last frozen span, for a total frozen of 10 + 10 + 10 + 2. We consume it all.
        // Frozen spans do not have to be full memory blocks!
        assert_eq!(consume32.detach_complete_frozen_spans, 4);
        assert_eq!(consume32.consume_partial_span_bytes, 0);
        assert_eq!(consume32.required_spans_capacity(), 4);
    }

    #[test]
    fn size_change_detector() {
        // The point of this is not to say that we expect it to have a specific size but to allow
        // us to easily detect when the size changes and (if we choose to) bless the change.
        // We assume 64-bit pointers - any support for 32-bit is problem for the future.
        assert_eq!(size_of::<BytesBuf>(), 552);
    }

    #[test]
    fn debug_fmt_mixed_state() {
        let memory = FixedBlockTestMemory::new(nz!(8));

        let mut builder = BytesBuf::new();

        // Reserve 2 blocks (16 bytes).
        // State: available=16, span_builders="8, 8"
        builder.reserve(16, &memory);

        // Write 12 bytes.
        // State: len=12, available=4, span_builders="8 + 0, 4 + 4"
        builder.put_u64(0xAAAA_AAAA_AAAA_AAAA);
        builder.put_u32(0xBBBB_BBBB);

        // Freeze the 12 bytes. This will create two frozen spans (8 and 4 bytes).
        // State: len=12, frozen=12, available=4, frozen_spans="8, 4", span_builders="4"
        builder.ensure_frozen(12);

        // Write 2 more bytes into the current span builder.
        // State: len=14, frozen=12, available=2, frozen_spans="8, 4", span_builders="2 + 2"
        builder.put_u16(0xCCCC);

        // Reserve another block.
        // State: len=14, frozen=12, available=10, frozen_spans="8, 4", span_builders="2 + 2, 8"
        builder.reserve(8, &memory);

        let debug_string = format!("{builder:?}");

        assert_eq!(
            debug_string,
            "BytesBuf { len: 14, frozen: 12, available: 10, frozen_spans: \"8, 4\", span_builders: \"2 + 2, 8\" }"
        );
    }

    #[test]
    fn peek_empty_builder() {
        let builder = BytesBuf::new();
        let peeked = builder.peek();
        
        assert!(peeked.is_empty());
        assert_eq!(peeked.len(), 0);
    }

    #[test]
    fn peek_with_frozen_spans_only() {
        let memory = FixedBlockTestMemory::new(nz!(10));
        let mut builder = BytesBuf::new();
        
        builder.reserve(20, &memory);
        builder.put_u64(0x1111_1111_1111_1111);
        builder.put_u64(0x2222_2222_2222_2222);
        
        // Both blocks are now frozen (filled completely)
        assert_eq!(builder.len(), 16);
        
        let mut peeked = builder.peek();
        
        assert_eq!(peeked.len(), 16);
        assert_eq!(peeked.get_u64(), 0x1111_1111_1111_1111);
        assert_eq!(peeked.get_u64(), 0x2222_2222_2222_2222);
        
        // Original builder still has the data
        assert_eq!(builder.len(), 16);
    }

    #[test]
    fn peek_with_partially_filled_span_builder() {
        let memory = FixedBlockTestMemory::new(nz!(10));
        let mut builder = BytesBuf::new();
        
        builder.reserve(10, &memory);
        builder.put_u64(0x3333_3333_3333_3333);
        builder.put_u16(0x4444);
        
        // We have 10 bytes filled in a 10-byte block
        assert_eq!(builder.len(), 10);
        
        let mut peeked = builder.peek();
        
        assert_eq!(peeked.len(), 10);
        assert_eq!(peeked.get_u64(), 0x3333_3333_3333_3333);
        assert_eq!(peeked.get_u16(), 0x4444);
        
        // Original builder still has the data
        assert_eq!(builder.len(), 10);
    }

    #[test]
    fn peek_preserves_capacity_of_partial_span_builder() {
        let memory = FixedBlockTestMemory::new(nz!(20));
        let mut builder = BytesBuf::new();
        
        builder.reserve(20, &memory);
        builder.put_u64(0x5555_5555_5555_5555);
        
        // We have 8 bytes filled and 12 bytes remaining capacity
        assert_eq!(builder.len(), 8);
        assert_eq!(builder.remaining_mut(), 12);
        
        let mut peeked = builder.peek();
        
        assert_eq!(peeked.len(), 8);
        assert_eq!(peeked.get_u64(), 0x5555_5555_5555_5555);
        
        // CRITICAL TEST: Capacity should be preserved
        assert_eq!(builder.len(), 8);
        assert_eq!(builder.remaining_mut(), 12);
        
        // We should still be able to write more data
        builder.put_u32(0x6666_6666);
        assert_eq!(builder.len(), 12);
        assert_eq!(builder.remaining_mut(), 8);
        
        // And we can peek again to see the updated data
        let mut peeked2 = builder.peek();
        assert_eq!(peeked2.len(), 12);
        assert_eq!(peeked2.get_u64(), 0x5555_5555_5555_5555);
        assert_eq!(peeked2.get_u32(), 0x6666_6666);
    }

    #[test]
    fn peek_with_mixed_frozen_and_unfrozen() {
        let memory = FixedBlockTestMemory::new(nz!(10));
        let mut builder = BytesBuf::new();
        
        builder.reserve(30, &memory);
        
        // Fill first block completely (10 bytes) - will be frozen
        builder.put_u64(0x1111_1111_1111_1111);
        builder.put_u16(0x2222);
        
        // Fill second block completely (10 bytes) - will be frozen
        builder.put_u64(0x3333_3333_3333_3333);
        builder.put_u16(0x4444);
        
        // Partially fill third block (only 4 bytes) - will remain unfrozen
        builder.put_u32(0x5555_5555);
        
        assert_eq!(builder.len(), 24);
        assert_eq!(builder.remaining_mut(), 6);
        
        let mut peeked = builder.peek();
        
        assert_eq!(peeked.len(), 24);
        assert_eq!(peeked.get_u64(), 0x1111_1111_1111_1111);
        assert_eq!(peeked.get_u16(), 0x2222);
        assert_eq!(peeked.get_u64(), 0x3333_3333_3333_3333);
        assert_eq!(peeked.get_u16(), 0x4444);
        assert_eq!(peeked.get_u32(), 0x5555_5555);
        
        // Original builder still has all the data and capacity
        assert_eq!(builder.len(), 24);
        assert_eq!(builder.remaining_mut(), 6);
    }

    #[test]
    fn peek_then_consume() {
        let memory = FixedBlockTestMemory::new(nz!(20));
        let mut builder = BytesBuf::new();
        
        builder.reserve(20, &memory);
        builder.put_u64(0x7777_7777_7777_7777);
        builder.put_u32(0x8888_8888);
        
        assert_eq!(builder.len(), 12);
        
        // Peek at the data
        let mut peeked = builder.peek();
        assert_eq!(peeked.len(), 12);
        assert_eq!(peeked.get_u64(), 0x7777_7777_7777_7777);
        
        // Original builder still has the data
        assert_eq!(builder.len(), 12);
        
        // Now consume some of it
        let mut consumed = builder.consume(8);
        assert_eq!(consumed.get_u64(), 0x7777_7777_7777_7777);
        
        // Builder should have less data now
        assert_eq!(builder.len(), 4);
        
        // Peek again should show the remaining data
        let mut peeked2 = builder.peek();
        assert_eq!(peeked2.len(), 4);
        assert_eq!(peeked2.get_u32(), 0x8888_8888);
    }

    #[test]
    fn peek_multiple_times() {
        let memory = FixedBlockTestMemory::new(nz!(20));
        let mut builder = BytesBuf::new();
        
        builder.reserve(20, &memory);
        builder.put_u64(0xAAAA_AAAA_AAAA_AAAA);
        
        // Peek multiple times - each should work independently
        let mut peeked1 = builder.peek();
        let mut peeked2 = builder.peek();
        
        assert_eq!(peeked1.get_u64(), 0xAAAA_AAAA_AAAA_AAAA);
        assert_eq!(peeked2.get_u64(), 0xAAAA_AAAA_AAAA_AAAA);
        
        // Original builder still intact
        assert_eq!(builder.len(), 8);
    }
}
