// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::mem::{self, MaybeUninit};
use std::num::NonZero;

use smallvec::SmallVec;

use crate::mem::{Block, BlockMeta, BlockSize, Memory};
use crate::{BytesBufWriter, BytesView, MAX_INLINE_SPANS, MemoryGuard, Span, SpanBuilder};

/// Assembles byte sequences, exposing them as [`BytesView`]s.
///
/// The buffer owns some memory capacity into which it allows you to write a sequence of bytes that
/// you can thereafter extract as one or more [`BytesView`]s over immutable data. Mutation of the
/// buffer contents is append-only - once data has been written into the buffer, it cannot be modified.
///
/// Capacity must be reserved in advance (e.g. via [`reserve()`]) before you can write data into the buffer.
/// The exception to this is when appending an existing [`BytesView`] via [`put_bytes()`] because
/// appending a [`BytesView`] is a zero-copy operation that reuses the view's existing memory capacity.
///
/// # Memory capacity
///
/// A single `BytesBuf` can use memory capacity from any [memory provider], including a
/// mix of different memory providers. All methods that extend the memory capacity require the caller
/// to provide a reference to the memory provider to use.
///
/// To understand how to obtain access to a memory provider, see [Producing Byte Sequences].
///
/// When data is extracted from the buffer by consuming it (via [`consume()`] or [`consume_all()`]),
/// ownership of the used memory capacity is transferred to the returned [`BytesView`]. Any leftover
/// memory capacity remains in the buffer, ready to receive further writes.
///
/// # Conceptual design
///
/// The memory capacity owned by a `BytesBuf` can be viewed as two regions:
///
/// * Filled memory - data has been written into this memory but this data has not yet been consumed as a
///   [`BytesView`]. Nevertheless, this data may already be in use because it may have been exposed via
///   [`peek()`], which does not consume it from the buffer. Memory capacity is removed from this region
///   when bytes are consumed from the buffer.
/// * Available memory - no data has been written into this memory. Calling any of the write methods on
///   `BytesBuf` will write data to the start of this region and transfer the affected capacity to the
///   filled memory region.
///
/// Existing [`BytesView`]s can be appended to the `BytesBuf` via [`put_bytes()`] without
/// consuming capacity as each appended [`BytesView`] brings its own backing memory capacity.
#[doc = include_str!("../doc/snippets/sequence_memory_layout.md")]
///
/// # Example
///
/// ```
/// # use bytesbuf::mem::{GlobalPool, Memory};
/// use bytesbuf::BytesBuf;
///
/// const HEADER_MAGIC: &[u8] = b"HDR\x00";
///
/// # let memory = GlobalPool::new();
/// let mut buf = memory.reserve(64);
///
/// // Build a message from various pieces.
/// buf.put_slice(HEADER_MAGIC);
/// buf.put_num_be(1_u16); // Version
/// buf.put_num_be(42_u32); // Payload length
/// buf.put_num_be(0xDEAD_BEEF_u64); // Checksum
///
/// // Consume the buffered data as an immutable BytesView.
/// let message = buf.consume_all();
/// assert_eq!(message.len(), 18);
/// ```
///
/// [memory provider]: crate::mem::Memory
/// [`reserve()`]: Self::reserve
/// [`put_bytes()`]: Self::put_bytes
/// [`consume()`]: Self::consume
/// [`consume_all()`]: Self::consume_all
/// [`peek()`]: Self::peek
/// [Producing Byte Sequences]: crate#producing-byte-sequences
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
    /// The total capacity is `len + available`.
    ///
    /// We cache this to avoid recalculating it every time we need this information.
    available: usize,
}

impl BytesBuf {
    /// Creates an instance without any memory capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an instance that owns the provided memory blocks.
    ///
    /// This is the API used by memory providers to issue rented memory capacity to callers.
    /// Unless you are implementing a memory provider, you will not need to call this function.
    /// Instead, use either [`Memory::reserve()`] or [`BytesBuf::reserve()`].
    ///
    /// # Blocks are unordered
    ///
    /// There is no guarantee that the `BytesBuf` uses the blocks in the order provided to
    /// this function. Blocks may be used in any order.
    ///
    /// [`Memory::reserve()`]: Memory::reserve
    /// [`BytesBuf::reserve()`]: Self::reserve
    #[must_use]
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

        let available = span_builders.iter().map(SpanBuilder::remaining_capacity).sum();

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

    /// Adds enough memory capacity to accommodate at least `additional_bytes` of content.
    ///
    /// After this call, [`remaining_capacity()`] will be at least `additional_bytes`.
    ///
    /// The memory provider may provide more capacity than requested - `additional_bytes` is only a lower bound.
    ///
    /// # Example
    ///
    /// ```
    /// use bytesbuf::BytesBuf;
    /// # use bytesbuf::mem::GlobalPool;
    ///
    /// # let memory = GlobalPool::new();
    /// let mut buf = BytesBuf::new();
    ///
    /// // Must reserve capacity before writing.
    /// buf.reserve(16, &memory);
    /// assert!(buf.remaining_capacity() >= 16);
    ///
    /// buf.put_num_be(0x1234_5678_u32);
    ///
    /// // Can reserve more capacity at any time.
    /// buf.reserve(100, &memory);
    /// assert!(buf.remaining_capacity() >= 100);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the resulting total buffer capacity would be greater than `usize::MAX`.
    ///
    /// [`remaining_capacity()`]: Self::remaining_capacity
    pub fn reserve<M: Memory + ?Sized>(&mut self, additional_bytes: usize, memory_provider: &M) {
        let bytes_needed = additional_bytes.saturating_sub(self.remaining_capacity());

        let Some(bytes_needed) = NonZero::new(bytes_needed) else {
            return;
        };

        self.extend_capacity_by_at_least(bytes_needed, memory_provider);
    }

    fn extend_capacity_by_at_least<M: Memory + ?Sized>(&mut self, bytes: NonZero<usize>, memory_provider: &M) {
        let additional_memory = memory_provider.reserve(bytes.get());

        // For extra paranoia. We expect a memory provider to return an empty buffer.
        debug_assert!(additional_memory.capacity() >= bytes.get());
        debug_assert!(additional_memory.is_empty());

        self.available = self
            .available
            .checked_add(additional_memory.capacity())
            .expect("buffer capacity cannot exceed usize::MAX");

        // We put the new ones in front (existing content needs to stay at the end).
        self.span_builders_reversed.insert_many(0, additional_memory.span_builders_reversed);
    }

    /// Appends the contents of an existing [`BytesView`] to the end of the buffer.
    ///
    /// Memory capacity of the existing [`BytesView`] is reused without copying.
    ///
    /// This is a private API to keep the nitty-gritty of span bookkeeping contained in this file
    /// while the public API lives in another file for ease of maintenance. The equivalent
    /// public API is `put_bytes()`.
    ///
    /// # Panics
    ///
    /// Panics if the resulting total buffer capacity would be greater than `usize::MAX`.
    pub(crate) fn append(&mut self, bytes: BytesView) {
        if bytes.is_empty() {
            return;
        }

        let bytes_len = bytes.len();

        // Only the first span builder may hold unfrozen data (the rest are for spare capacity).
        let total_unfrozen_bytes = NonZero::new(self.span_builders_reversed.last().map_or(0, SpanBuilder::len));

        if let Some(total_unfrozen_bytes) = total_unfrozen_bytes {
            // If there is any unfrozen data, we freeze it now to ensure we append after all
            // existing data already in the sequence builder.
            self.freeze_from_first(total_unfrozen_bytes);

            // Debug build paranoia: nothing remains in the span builder, right?
            debug_assert!(self.span_builders_reversed.last().map_or(0, SpanBuilder::len) == 0);
        }

        // We do this first so if we do panic, we have not performed any incomplete operations.
        // The freezing above is safe even if we panic here - freezing is an atomic operation.
        self.len = self.len.checked_add(bytes_len).expect("buffer capacity cannot exceed usize::MAX");

        // Any appended BytesView is frozen by definition, as contents of a BytesView are immutable.
        // This cannot wrap because we verified `len` is in-bounds and `frozen <= len` is a type invariant.
        self.frozen = self.frozen.wrapping_add(bytes_len);

        self.frozen_spans.extend(bytes.into_spans_reversed().into_iter().rev());
    }

    /// Peeks at the contents of the filled bytes region.
    ///
    /// The returned [`BytesView`] covers all data in the buffer but does not consume any of the data.
    ///
    /// Functionally similar to [`consume_all()`] except all the data remains in the
    /// buffer and can still be consumed later.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(16);
    /// buf.put_num_be(0x1234_u16);
    /// buf.put_num_be(0x5678_u16);
    ///
    /// // Peek at the data without consuming it.
    /// let mut peeked = buf.peek();
    /// assert_eq!(peeked.get_num_be::<u16>(), 0x1234);
    /// assert_eq!(peeked.get_num_be::<u16>(), 0x5678);
    ///
    /// // Despite consuming from peeked, the buffer still contains all data.
    /// assert_eq!(buf.len(), 4);
    ///
    /// let consumed = buf.consume_all();
    /// assert_eq!(consumed.len(), 4);
    /// ```
    ///
    /// [`consume_all()`]: Self::consume_all
    #[must_use]
    pub fn peek(&self) -> BytesView {
        // Build a list of all spans to include in the result, in reverse order for efficient construction.
        let mut result_spans_reversed: SmallVec<[Span; MAX_INLINE_SPANS]> = SmallVec::new();

        // Add any filled data from the first (potentially partially filled) span builder.
        if let Some(first_builder) = self.span_builders_reversed.last() {
            // We only peek the span builder, as well. This is to avoid freezing it because freezing
            // has security/performance implications and the motivating idea behind peeking is to
            // verify the contents are ready for processing before we commit to freezing them.
            let span = first_builder.peek();

            // It might just be empty - that's also fine.
            if !span.is_empty() {
                result_spans_reversed.push(span);
            }
        }

        // Add all the frozen spans. They are stored in content order in our storage,
        // so we reverse them when adding to the result_spans_reversed collection.
        result_spans_reversed.extend(self.frozen_spans.iter().rev().cloned());

        BytesView::from_spans_reversed(result_spans_reversed)
    }

    /// How many bytes of data are in the buffer, ready to be consumed.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(32);
    /// assert_eq!(buf.len(), 0);
    ///
    /// buf.put_num_be(0x1234_5678_u32);
    /// assert_eq!(buf.len(), 4);
    ///
    /// buf.put_slice(*b"Hello");
    /// assert_eq!(buf.len(), 9);
    ///
    /// _ = buf.consume(4);
    /// assert_eq!(buf.len(), 5);
    /// ```
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

        // Will not overflow - `capacity <= usize::MAX` is a type invariant and obviously `len < capacity`.
        frozen_len.wrapping_add(unfrozen_len)
    }

    /// Whether the buffer is empty (contains no data).
    ///
    /// This does not imply that the buffer has no remaining memory capacity.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The total capacity of the buffer.
    ///
    /// This is the total length of the filled bytes and the available bytes regions.
    ///
    /// # Example
    ///
    /// ```
    /// use bytesbuf::BytesBuf;
    /// # use bytesbuf::mem::GlobalPool;
    ///
    /// # let memory = GlobalPool::new();
    /// let mut buf = BytesBuf::new();
    /// assert_eq!(buf.capacity(), 0);
    ///
    /// buf.reserve(100, &memory);
    /// let initial_capacity = buf.capacity();
    /// assert!(initial_capacity >= 100);
    ///
    /// // Writing does not change capacity.
    /// buf.put_slice(*b"Hello");
    /// assert_eq!(buf.capacity(), initial_capacity);
    ///
    /// // Consuming reduces capacity (memory is transferred to the BytesView).
    /// _ = buf.consume(5);
    /// assert!(buf.capacity() < initial_capacity);
    /// ```
    #[must_use]
    pub fn capacity(&self) -> usize {
        // Will not overflow - `capacity <= usize::MAX` is a type invariant.
        self.len().wrapping_add(self.remaining_capacity())
    }

    /// How many more bytes can be written into the buffer
    /// before its memory capacity is exhausted.
    ///
    /// # Example
    ///
    /// ```
    /// use bytesbuf::BytesBuf;
    /// # use bytesbuf::mem::GlobalPool;
    ///
    /// # let memory = GlobalPool::new();
    /// let mut buf = BytesBuf::new();
    ///
    /// buf.reserve(100, &memory);
    /// let initial_remaining = buf.remaining_capacity();
    /// assert!(initial_remaining >= 100);
    ///
    /// // Writing reduces remaining capacity.
    /// buf.put_slice(*b"Hello");
    /// assert_eq!(buf.remaining_capacity(), initial_remaining - 5);
    ///
    /// // Reserving more increases remaining capacity.
    /// buf.reserve(200, &memory);
    /// assert!(buf.remaining_capacity() >= 200);
    ///
    /// // Consuming buffered data does NOT affect remaining capacity.
    /// let remaining_before_consume = buf.remaining_capacity();
    /// _ = buf.consume(5);
    /// assert_eq!(buf.remaining_capacity(), remaining_before_consume);
    /// ```
    #[cfg_attr(test, mutants::skip)] // Lying about buffer sizes is an easy way to infinite loops.
    pub fn remaining_capacity(&self) -> usize {
        // The remaining capacity is the sum of the remaining capacity of all span builders.
        debug_assert_eq!(
            self.available,
            self.span_builders_reversed
                .iter()
                .map(SpanBuilder::remaining_capacity)
                .sum::<usize>()
        );

        self.available
    }

    /// Consumes `len` bytes from the beginning of the buffer.
    ///
    /// The consumed bytes and the memory capacity that backs them are removed from the buffer.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(32);
    ///
    /// buf.put_num_be(0x1111_u16);
    /// buf.put_num_be(0x2222_u16);
    ///
    /// // Consume first part.
    /// let mut first = buf.consume(2);
    /// assert_eq!(first.get_num_be::<u16>(), 0x1111);
    ///
    /// // Write more data.
    /// buf.put_num_be(0x3333_u16);
    ///
    /// // Consume remaining data.
    /// let mut rest = buf.consume(4);
    /// assert_eq!(rest.get_num_be::<u16>(), 0x2222);
    /// assert_eq!(rest.get_num_be::<u16>(), 0x3333);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the buffer does not contain at least `len` bytes.
    pub fn consume(&mut self, len: usize) -> BytesView {
        self.consume_checked(len)
            .expect("attempted to consume more bytes than available in buffer")
    }

    /// Consumes `len` bytes from the beginning of the buffer.
    ///
    /// Returns `None` if the buffer does not contain at least `len` bytes.
    ///
    /// The consumed bytes and the memory capacity that backs them are removed from the buffer.
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    #[cfg_attr(test, mutants::skip)] // Mutating the bounds check causes UB via unwrap_unchecked in consume_all or infinite loops in prepare_consume.
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

            // SAFETY: We must guarantee that we do not try to advance out of bounds. This is guaranteed
            // by the manifest calculation, the job of which is to determine the right in-bounds value.
            unsafe { partially_consumed_frozen_span.advance(manifest.consume_partial_span_bytes as usize) };
        }

        // We extend the result spans with the (storage-order) fully detached spans.
        // BytesBuf stores the frozen spans in content order, so we must reverse.
        result_spans_reversed.extend(self.frozen_spans.drain(..manifest.detach_complete_frozen_spans).rev());

        // Will not wrap because we verified bounds above.
        self.len = self.len.wrapping_sub(len);

        // Will not wrap because all consumed data must first have been frozen,
        // which we guarantee via ensure_frozen() above.
        self.frozen = self.frozen.wrapping_sub(len);

        Some(BytesView::from_spans_reversed(result_spans_reversed))
    }

    fn prepare_consume(&self, mut len: usize) -> ConsumeManifest {
        debug_assert!(len <= self.frozen);

        let mut detach_complete_frozen_spans: usize = 0;

        for span in &self.frozen_spans {
            let span_len = span.len();

            if span_len as usize <= len {
                // Will not wrap because a type invariant is `capacity <= usize::MAX`, so if
                // capacity is in-bounds, the number of spans could not possibly be greater.
                detach_complete_frozen_spans = detach_complete_frozen_spans.wrapping_add(1);

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
            consume_partial_span_bytes: len.try_into().expect("we are supposed to have less than one memory block worth of data remaining but its length does not fit into a single memory block - algorithm defect"),
        }
    }

    /// Consumes all bytes in the buffer.
    ///
    /// The consumed bytes and the memory capacity that backs them are removed from the buffer.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(32);
    /// buf.put_slice(*b"Hello, ");
    /// buf.put_slice(*b"world!");
    /// buf.put_num_be(0x2121_u16); // "!!"
    ///
    /// let message = buf.consume_all();
    ///
    /// assert_eq!(message, b"Hello, world!!!");
    /// assert!(buf.is_empty());
    /// ```
    pub fn consume_all(&mut self) -> BytesView {
        // SAFETY: Consuming len() bytes from self cannot possibly be out of bounds.
        unsafe { self.consume_checked(self.len()).unwrap_unchecked() }
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

        if span_builder.remaining_capacity() == 0 {
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

    /// The first slice of memory in the remaining capacity of the buffer.
    ///
    /// This allows you to manually write into the buffer instead of using the various
    /// provided convenience methods. Only the first slice of the remaining capacity is
    /// exposed at any given time by this API.
    ///
    /// After writing data to the start of this slice, call [`advance()`] to indicate
    /// how many bytes have been filled with data. The next call to `first_unfilled_slice()`
    /// will return the next slice of memory you can write into. This slice must be
    /// completely filled before the next slice is exposed (a partial fill will simply
    /// return the remaining range from the same slice in the next call).
    ///
    /// To write to multiple slices concurrently, use [`begin_vectored_write()`].
    #[doc = include_str!("../doc/snippets/sequence_memory_layout.md")]
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(64);
    /// let data_to_write: &[u8] = b"0123456789";
    ///
    /// // Write data without assuming the length of first_unfilled_slice().
    /// let mut written = 0;
    ///
    /// while written < data_to_write.len() {
    ///     let dst = buf.first_unfilled_slice();
    ///
    ///     let bytes_to_write = dst.len().min(data_to_write.len() - written);
    ///
    ///     for i in 0..bytes_to_write {
    ///         dst[i].write(data_to_write[written + i]);
    ///     }
    ///
    ///     // SAFETY: We just initialized `bytes_to_write` bytes.
    ///     unsafe {
    ///         buf.advance(bytes_to_write);
    ///     }
    ///     written += bytes_to_write;
    /// }
    ///
    /// assert_eq!(buf.consume_all(), b"0123456789");
    /// ```
    ///
    /// [`advance()`]: Self::advance
    /// [`begin_vectored_write()`]: Self::begin_vectored_write
    pub fn first_unfilled_slice(&mut self) -> &mut [MaybeUninit<u8>] {
        if let Some(last) = self.span_builders_reversed.last_mut() {
            last.unfilled_slice_mut()
        } else {
            // We are required to always return something, even if we have no span builders!
            &mut []
        }
    }

    /// Inspects the metadata of the memory block backing [`first_unfilled_slice()`].
    ///
    /// `None` if there is no metadata associated with the memory block or
    /// if the buffer has no remaining capacity.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// # struct PageAlignedMemory;
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(64);
    ///
    /// let is_page_aligned = buf
    ///     .first_unfilled_slice_meta()
    ///     .is_some_and(|meta| meta.is::<PageAlignedMemory>());
    ///
    /// println!("First unfilled slice is page-aligned: {is_page_aligned}");
    /// ```
    ///
    /// [`first_unfilled_slice()`]: Self::first_unfilled_slice
    #[must_use]
    pub fn first_unfilled_slice_meta(&self) -> Option<&dyn BlockMeta> {
        self.span_builders_reversed.last().and_then(|sb| sb.block().meta())
    }

    /// Signals that `count` bytes have been written to the start of [`first_unfilled_slice()`].
    ///
    /// The next call to [`first_unfilled_slice()`] will return the next slice of memory that
    /// can be filled with data.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(64);
    /// let data_to_write: &[u8] = b"0123456789";
    ///
    /// // Write data without assuming the length of first_unfilled_slice().
    /// let mut written = 0;
    ///
    /// while written < data_to_write.len() {
    ///     let dst = buf.first_unfilled_slice();
    ///
    ///     let bytes_to_write = dst.len().min(data_to_write.len() - written);
    ///
    ///     for i in 0..bytes_to_write {
    ///         dst[i].write(data_to_write[written + i]);
    ///     }
    ///
    ///     // SAFETY: We just initialized `bytes_to_write` bytes.
    ///     unsafe {
    ///         buf.advance(bytes_to_write);
    ///     }
    ///     written += bytes_to_write;
    /// }
    ///
    /// assert_eq!(buf.consume_all(), b"0123456789");
    /// ```
    ///
    /// # Safety
    ///
    /// The caller must guarantee that `count` bytes from the beginning of [`first_unfilled_slice()`]
    /// have been initialized.
    ///
    /// [`first_unfilled_slice()`]: Self::first_unfilled_slice
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub unsafe fn advance(&mut self, count: usize) {
        if count == 0 {
            return;
        }

        // The write head can only be advanced (via this method) up to the end of the first slice, no further.
        // This is guaranteed by our safety requirements, so we only assert this in debug builds for extra validation.
        debug_assert!(count <= self.span_builders_reversed.last().map_or(0, SpanBuilder::remaining_capacity));

        let span_builder = self
            .span_builders_reversed
            .last_mut()
            .expect("there must be at least one span builder if we wrote nonzero bytes");

        // SAFETY: We simply rely on the caller's safety promises here, "forwarding" them.
        unsafe { span_builder.advance(count) };

        if span_builder.remaining_capacity() == 0 {
            // The span builder is full, so we need to freeze it and move it to the frozen spans.
            let len = NonZero::new(span_builder.len())
                .expect("there is no capacity left in the span builder so there must be at least one byte to consume unless we somehow left an empty span builder in the queue");

            self.freeze_from_first(len);

            // Debug build paranoia: no full span remains after freeze, right?
            debug_assert!(
                self.span_builders_reversed
                    .last()
                    .map_or(usize::MAX, SpanBuilder::remaining_capacity)
                    > 0
            );
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

    /// Concurrently writes data into all the byte slices that make up the buffer.
    ///
    /// The vectored write takes exclusive ownership of the buffer for the duration of the operation
    /// and allows individual slices of the remaining capacity to be filled concurrently, up to an
    /// optional limit of `max_len` bytes.
    ///
    /// Some I/O operations are naturally limited to a maximum number of bytes that can be
    /// transferred, so the length limit here allows you to project a restricted view of the
    /// available capacity without having to limit the true capacity of the buffer.
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use std::ptr;
    ///
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(64);
    /// let capacity = buf.remaining_capacity();
    ///
    /// let mut vectored = buf.begin_vectored_write(None);
    /// let mut slices: Vec<_> = vectored.slices_mut().map(|(s, _)| s).collect();
    ///
    /// // Fill all slices with 0xAE bytes.
    /// // In practice, these could be filled concurrently by vectored I/O APIs.
    /// let mut total_written = 0;
    /// for slice in &mut slices {
    ///     // SAFETY: Writing valid u8 values to the entire slice.
    ///     unsafe {
    ///         ptr::write_bytes(slice.as_mut_ptr(), 0xAE, slice.len());
    ///     }
    ///     total_written += slice.len();
    /// }
    ///
    /// // SAFETY: We initialized `total_written` bytes sequentially.
    /// unsafe {
    ///     vectored.commit(total_written);
    /// }
    ///
    /// assert_eq!(buf.len(), capacity);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `max_len` is greater than the remaining capacity of the buffer.
    pub fn begin_vectored_write(&mut self, max_len: Option<usize>) -> BytesBufVectoredWrite<'_> {
        self.begin_vectored_write_checked(max_len)
            .expect("attempted to begin a vectored write with a max_len that was greater than the remaining capacity")
    }

    /// Concurrently writes data into all the byte slices that make up the buffer.
    ///
    /// The vectored write takes exclusive ownership of the buffer for the duration of the operation
    /// and allows individual slices of the remaining capacity to be filled concurrently, up to an
    /// optional limit of `max_len` bytes.
    ///
    /// Some I/O operations are naturally limited to a maximum number of bytes that can be
    /// transferred, so the length limit here allows you to project a restricted view of the
    /// available capacity without having to limit the true capacity of the buffer.
    ///
    /// # Returns
    ///
    /// Returns `None` if `max_len` is greater than the remaining capacity of the buffer.
    pub fn begin_vectored_write_checked(&mut self, max_len: Option<usize>) -> Option<BytesBufVectoredWrite<'_>> {
        if let Some(max_len) = max_len
            && max_len > self.remaining_capacity()
        {
            return None;
        }

        Some(BytesBufVectoredWrite { buf: self, max_len })
    }

    fn iter_available_capacity(&mut self, max_len: Option<usize>) -> BytesBufRemaining<'_> {
        let next_span_builder_index = if self.span_builders_reversed.is_empty() { None } else { Some(0) };

        BytesBufRemaining {
            buf: self,
            next_span_builder_index,
            max_len,
        }
    }

    /// Extends the lifetime of the memory capacity backing this buffer.
    ///
    /// This can be useful when unsafe code is used to reference the contents of a `BytesBuf` and it
    /// is possible to reach a condition where the `BytesBuf` itself no longer exists, even though
    /// the contents are referenced (e.g. because the remaining references are in non-Rust code).
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
    ///
    /// # Example
    ///
    /// ```
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use std::io::Write;
    ///
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(32);
    /// {
    ///     let mut writer = buf.writer(&memory);
    ///     writer.write_all(b"Hello, ")?;
    ///     writer.write_all(b"world!")?;
    /// }
    ///
    /// assert_eq!(buf.consume_all(), b"Hello, world!");
    /// # Ok::<(), std::io::Error>(())
    /// ```
    #[inline]
    pub fn writer<'m, M: Memory + ?Sized>(&mut self, memory: &'m M) -> BytesBufWriter<'_, 'm, M> {
        BytesBufWriter::new(self, memory)
    }
}

impl std::fmt::Debug for BytesBuf {
    #[cfg_attr(test, mutants::skip)] // We have no API contract here.
    #[cfg_attr(coverage_nightly, coverage(off))] // We have no API contract here.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let frozen_spans = self.frozen_spans.iter().map(|x| x.len().to_string()).collect::<Vec<_>>().join(", ");

        let span_builders = self
            .span_builders_reversed
            .iter()
            .rev()
            .map(|x| {
                if x.is_empty() {
                    x.remaining_capacity().to_string()
                } else {
                    format!("{} + {}", x.len(), x.remaining_capacity())
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        f.debug_struct(type_name::<Self>())
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
            // This will not wrap because a type invariant is `capacity <= usize::MAX`, so if
            // capacity is already in-bounds, the count of spans certainly is not a greater number.
            self.detach_complete_frozen_spans.wrapping_add(1)
        } else {
            self.detach_complete_frozen_spans
        }
    }
}

/// Coordinates concurrent write operations into a buffer's memory capacity.
///
/// The operation takes exclusive ownership of the `BytesBuf`. During the vectored write,
/// the remaining capacity of the `BytesBuf` is exposed as `MaybeUninit<u8>` slices
/// that at the end of the operation must be filled sequentially and in order, without gaps,
/// in any desired amount (from 0 bytes written to all slices filled).
///
/// All slices may be written to concurrently and/or in any order - consistency of the contents
/// is only required at the moment the write is committed.
///
/// The capacity exposed during the operation can optionally be limited to `max_len` bytes.
///
/// The operation is completed by calling `.commit()` on the instance, after which the operation is
/// consumed and the exclusive ownership of the `BytesBuf` released.
///
/// If the instance is dropped without committing, the operation is aborted and all remaining capacity
/// is left in a potentially uninitialized state.
#[derive(Debug)]
pub struct BytesBufVectoredWrite<'a> {
    buf: &'a mut BytesBuf,
    max_len: Option<usize>,
}

impl BytesBufVectoredWrite<'_> {
    /// Iterates over the slices of available capacity of the buffer,
    /// together with the metadata of the memory block backing each slice.
    ///
    /// The slices returned from this iterator have the lifetime of the vectored
    /// write operation itself, allowing them to be mutated concurrently.
    pub fn slices_mut(&mut self) -> BytesBufRemaining<'_> {
        self.buf.iter_available_capacity(self.max_len)
    }

    /// Extends the lifetime of the memory capacity backing this buffer.
    ///
    /// This can be useful when unsafe code is used to reference the contents of a `BytesBuf` and it
    /// is possible to reach a condition where the `BytesBuf` itself no longer exists, even though
    /// the contents are referenced (e.g. because the remaining references are in non-Rust code).
    pub fn extend_lifetime(&self) -> MemoryGuard {
        self.buf.extend_lifetime()
    }

    /// Completes the vectored write operation, committing `bytes_written` bytes of data that
    /// sequentially and completely fills slices from the start of the provided slices.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `bytes_written` bytes of data have actually been written
    /// into the slices of memory returned from `slices_mut()`, sequentially from the start.
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub unsafe fn commit(self, bytes_written: usize) {
        debug_assert!(bytes_written <= self.buf.remaining_capacity());

        if let Some(max_len) = self.max_len {
            debug_assert!(bytes_written <= max_len);
        }

        // Ordinarily, we have a type invariant that only the first span builder may contain data,
        // with the others being spare capacity. For the duration of a vectored write, this
        // invariant is suspended (because the vectored write has an exclusive reference which makes
        // the suspension of this invariant invisible to any other caller). We must now restore this
        // invariant. We do this by advancing the write head slice by slice, triggering the normal
        // freezing logic as we go (to avoid implementing two versions of the same logic), until we
        // have run out of written bytes to commit.

        let mut bytes_remaining = bytes_written;

        while bytes_remaining > 0 {
            let span_builder = self
                .buf
                .span_builders_reversed
                .last_mut()
                .expect("there must be at least one span builder because we still have filled capacity remaining to freeze");

            let bytes_available = span_builder.remaining_capacity();
            let bytes_to_commit = bytes_available.min(bytes_remaining);

            // SAFETY: We forward the promise from our own safety requirements to guarantee that
            // the specified number of bytes really has been written.
            unsafe { self.buf.advance(bytes_to_commit) };

            bytes_remaining = bytes_remaining
                .checked_sub(bytes_to_commit)
                .expect("we somehow advanced the write head more than the count of written bytes");
        }
    }
}

/// Exposes the remaining memory capacity of a `BytesBuf` for concurrent writes.
///
/// This is used during a vectored write operation, iterating over a sequence
/// of `MaybeUninit<u8>` slices that the caller can concurrently write into.
///
/// The slices may be mutated for as long as the vectored write operation exists.
#[derive(Debug)]
pub struct BytesBufRemaining<'a> {
    buf: &'a mut BytesBuf,
    next_span_builder_index: Option<usize>,

    // Self-imposed constraint on how much of the available capacity is made visible through
    // this iterator. This can be useful to limit the amount of data that can be written into
    // a `BytesBuf` during a vectored write operation without having to limit the
    // actual capacity of the `BytesBuf`.
    max_len: Option<usize>,
}

impl<'a> Iterator for BytesBufRemaining<'a> {
    type Item = (&'a mut [MaybeUninit<u8>], Option<&'a dyn BlockMeta>);

    #[cfg_attr(test, mutants::skip)] // This gets mutated into an infinite loop which is not very helpful.
    fn next(&mut self) -> Option<Self::Item> {
        let next_span_builder_index = self.next_span_builder_index?;

        self.next_span_builder_index = Some(
            // Will not overflow because `capacity <= usize::MAX` is a type invariant,
            // so the count of span builders certainly cannot be greater.
            next_span_builder_index.wrapping_add(1),
        );
        if self.next_span_builder_index == Some(self.buf.span_builders_reversed.len()) {
            self.next_span_builder_index = None;
        }

        // The iterator iterates through things in content order but we need to access
        // the span builders in storage order.
        let next_span_builder_index_storage_order = self
            .buf
            .span_builders_reversed
            .len()
            // Will not overflow because `capacity <= usize::MAX` is a type invariant,
            // so the count of span builders certainly cannot be greater.
            .wrapping_sub(next_span_builder_index + 1);

        let span_builder = self
            .buf
            .span_builders_reversed
            .get_mut(next_span_builder_index_storage_order)
            .expect("iterator cursor referenced a span builder that does not exist");

        let meta_with_a = {
            let meta = span_builder.block().meta();

            // SAFETY: The metadata reference points into the block's heap allocation, not into
            // the span builder's stack memory. We transmute it to 'a immediately so the immutable
            // borrow of `span_builder` is released before the mutable borrow below.
            // The metadata is valid for 'a because the BlockRef implementation guarantees metadata
            // lives as long as any clone of the memory block, and we hold an exclusive reference
            // to the BytesBuf for the lifetime 'a.
            unsafe { mem::transmute::<Option<&dyn BlockMeta>, Option<&'a dyn BlockMeta>>(meta) }
        };

        let uninit_slice_mut = span_builder.unfilled_slice_mut();

        // SAFETY: There is nothing Rust can do to promise the reference we return is valid for 'a
        // but we can make such a promise ourselves. In essence, returning the references with 'a
        // this will extend the exclusive ownership of `BytesBuf` until all returned chunk
        // references are dropped, even if the iterator itself is dropped earlier. We can do this
        // because we know that to access the chunks requires a reference to the `BytesBuf`,
        // so as long as a chunk reference exists, access via the `BytesBuf` is blocked.
        let uninit_slice_mut = unsafe { mem::transmute::<&mut [MaybeUninit<u8>], &'a mut [MaybeUninit<u8>]>(&mut *uninit_slice_mut) };

        let uninit_slice_mut = if let Some(max_len) = self.max_len {
            // Limit the visible range of the slice if we have a size limit.
            // If this results in the slice being limited to not its full size,
            // we will also terminate the iteration
            let constrained_len = uninit_slice_mut.len().min(max_len);

            let adjusted_slice = uninit_slice_mut.get_mut(..constrained_len).expect("guarded by min() above");

            // Will not wrap because it is guarded by min() above.
            self.max_len = Some(max_len.wrapping_sub(constrained_len));

            if self.max_len == Some(0) {
                // Even if there are more span builders, we have returned all the capacity
                // we are allowed to return, so pretend there is nothing more to return.
                self.next_span_builder_index = None;
            }

            adjusted_slice
        } else {
            uninit_slice_mut
        };

        Some((uninit_slice_mut, meta_with_a))
    }
}

impl From<BytesView> for BytesBuf {
    fn from(value: BytesView) -> Self {
        let mut buf = Self::new();
        buf.append(value);
        buf
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "Fine in test code, we prefer panic on error")]

    use std::pin::pin;

    use new_zealand::nz;
    use static_assertions::assert_impl_all;
    use testing_aids::assert_panic;

    use super::*;
    use crate::mem::GlobalPool;
    use crate::mem::testing::{FixedBlockMemory, TestMemoryBlock};

    const U64_SIZE: usize = size_of::<u64>();
    const TWO_U64_SIZE: usize = size_of::<u64>() + size_of::<u64>();
    const THREE_U64_SIZE: usize = size_of::<u64>() + size_of::<u64>() + size_of::<u64>();

    assert_impl_all!(BytesBuf: Send, Sync);

    #[test]
    fn smoke_test() {
        let memory = FixedBlockMemory::new(nz!(1234));

        let min_length = 1000;

        let mut buf = memory.reserve(min_length);

        assert!(buf.capacity() >= min_length);
        assert!(buf.remaining_capacity() >= min_length);
        assert_eq!(buf.capacity(), buf.remaining_capacity());
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());

        buf.put_num_ne(1234_u64);
        buf.put_num_ne(5678_u64);
        buf.put_num_ne(1234_u64);
        buf.put_num_ne(5678_u64);

        assert_eq!(buf.len(), 32);
        assert!(!buf.is_empty());

        // SAFETY: Writing 0 bytes is always valid.
        unsafe {
            buf.advance(0);
        }

        let mut first_two = buf.consume(TWO_U64_SIZE);
        let mut second_two = buf.consume(TWO_U64_SIZE);

        assert_eq!(first_two.len(), 16);
        assert_eq!(second_two.len(), 16);
        assert_eq!(buf.len(), 0);

        assert_eq!(first_two.get_num_ne::<u64>(), 1234);
        assert_eq!(first_two.get_num_ne::<u64>(), 5678);

        assert_eq!(second_two.get_num_ne::<u64>(), 1234);
        assert_eq!(second_two.get_num_ne::<u64>(), 5678);

        buf.put_num_ne(1111_u64);

        assert_eq!(buf.len(), 8);

        let mut last = buf.consume(U64_SIZE);

        assert_eq!(last.len(), 8);
        assert_eq!(buf.len(), 0);

        assert_eq!(last.get_num_ne::<u64>(), 1111);

        assert!(buf.consume_checked(1).is_none());
        assert!(buf.consume_all().is_empty());
    }

    #[test]
    fn extend_capacity() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);

        let memory = FixedBlockMemory::new(nz!(100));

        // Have 0, desired 10, requesting 10, will get 100.
        buf.reserve(10, &memory);

        assert_eq!(buf.capacity(), 100);
        assert_eq!(buf.remaining_capacity(), 100);

        // Write 10 bytes of data just to verify that it does not affect "capacity" logic.
        buf.put_num_ne(1234_u64);
        buf.put_num_ne(5678_u16);

        assert_eq!(buf.len(), 10);
        assert_eq!(buf.remaining_capacity(), 90);
        assert_eq!(buf.capacity(), 100);

        // Have 100, desired 10+140=150, requesting 50, will get another 100 for a total of 200.
        buf.reserve(140, &memory);

        assert_eq!(buf.len(), 10);
        assert_eq!(buf.remaining_capacity(), 190);
        assert_eq!(buf.capacity(), 200);

        // Have 200, desired 10+200=210, 210-200=10, will get another 100.
        buf.reserve(200, &memory);

        assert_eq!(buf.len(), 10);
        assert_eq!(buf.remaining_capacity(), 290);
        assert_eq!(buf.capacity(), 300);
    }

    #[test]
    fn append_existing_view() {
        let memory = FixedBlockMemory::new(nz!(1234));

        let min_length = 1000;

        // This one we use to prepare some data to append.
        let mut payload_buffer = memory.reserve(min_length);

        // This is where we append the data to.
        let mut target_buffer = memory.reserve(min_length);

        // First we make a couple pieces to append.
        payload_buffer.put_num_ne(1111_u64);
        payload_buffer.put_num_ne(2222_u64);
        payload_buffer.put_num_ne(3333_u64);
        payload_buffer.put_num_ne(4444_u64);

        let payload1 = payload_buffer.consume(TWO_U64_SIZE);
        let payload2 = payload_buffer.consume(TWO_U64_SIZE);

        // Then we prefill some data to start us off.
        target_buffer.put_num_ne(5555_u64);
        target_buffer.put_num_ne(6666_u64);

        // Consume a little just for extra complexity.
        let _ = target_buffer.consume(U64_SIZE);

        // Append the payloads.
        target_buffer.put_bytes(payload1);
        target_buffer.put_bytes(payload2);

        // Appending an empty byte sequence does nothing.
        target_buffer.put_bytes(BytesView::default());

        // Add some custom data at the end.
        target_buffer.put_num_ne(7777_u64);

        assert_eq!(target_buffer.len(), 48);

        let mut result = target_buffer.consume(48);

        assert_eq!(result.get_num_ne::<u64>(), 6666);
        assert_eq!(result.get_num_ne::<u64>(), 1111);
        assert_eq!(result.get_num_ne::<u64>(), 2222);
        assert_eq!(result.get_num_ne::<u64>(), 3333);
        assert_eq!(result.get_num_ne::<u64>(), 4444);
        assert_eq!(result.get_num_ne::<u64>(), 7777);
    }

    #[test]
    fn consume_all_mixed() {
        let mut buf = BytesBuf::new();
        let memory = FixedBlockMemory::new(nz!(8));

        // Reserve some capacity and add initial data.
        buf.reserve(16, &memory);
        buf.put_num_ne(1111_u64);
        buf.put_num_ne(2222_u64);

        // Consume some data (the 1111).
        let _ = buf.consume(8);

        // Append a sequence (the 3333).
        let mut append_buf = BytesBuf::new();
        append_buf.reserve(8, &memory);
        append_buf.put_num_ne(3333_u64);
        let reused_bytes_to_append = append_buf.consume_all();
        buf.append(reused_bytes_to_append);

        // Add more data (the 4444).
        buf.reserve(8, &memory);
        buf.put_num_ne(4444_u64);

        // Consume all data and validate we got all the pieces.
        let mut result = buf.consume_all();

        assert_eq!(result.len(), 24);
        assert_eq!(result.get_num_ne::<u64>(), 2222);
        assert_eq!(result.get_num_ne::<u64>(), 3333);
        assert_eq!(result.get_num_ne::<u64>(), 4444);
    }

    #[test]
    #[expect(clippy::cognitive_complexity, reason = "test code")]
    fn peek_basic() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);

        let memory = FixedBlockMemory::new(nz!(10));

        // Peeking an empty buffer is fine, it is just an empty BytesView in that case.
        let peeked = buf.peek();
        assert_eq!(peeked.len(), 0);

        buf.reserve(100, &memory);

        assert_eq!(buf.capacity(), 100);

        buf.put_num_ne(1111_u64);

        // We have 0 frozen spans and 10 span builders,
        // the first of which has 8 bytes of filled content.
        let mut peeked = buf.peek();
        assert_eq!(peeked.first_slice().len(), 8);
        assert_eq!(peeked.get_num_ne::<u64>(), 1111);
        assert_eq!(peeked.len(), 0);

        buf.put_num_ne(2222_u64);
        buf.put_num_ne(3333_u64);
        buf.put_num_ne(4444_u64);
        buf.put_num_ne(5555_u64);
        buf.put_num_ne(6666_u64);
        buf.put_num_ne(7777_u64);
        buf.put_num_ne(8888_u64);
        // These will cross a span boundary so we can also observe
        // crossing that boundary during peeking.
        buf.put_byte_repeated(9, 8);

        assert_eq!(buf.len(), 72);
        assert_eq!(buf.capacity(), 100);
        assert_eq!(buf.remaining_capacity(), 28);

        // We should have 7 frozen spans and 3 span builders,
        // the first of which has 2 bytes of filled content.
        let mut peeked = buf.peek();

        assert_eq!(peeked.len(), 72);

        // This should be the first frozen span of 10 bytes.
        assert_eq!(peeked.first_slice().len(), 10);

        assert_eq!(peeked.get_num_ne::<u64>(), 1111);
        assert_eq!(peeked.get_num_ne::<u64>(), 2222);

        // The length of the buffer does not change just because we peek at its data.
        assert_eq!(buf.len(), 72);

        // We consumed 16 bytes from the peeked view, so should be looking at the remaining 4 bytes in the 2nd span.
        assert_eq!(peeked.first_slice().len(), 4);

        assert_eq!(peeked.get_num_ne::<u64>(), 3333);
        assert_eq!(peeked.get_num_ne::<u64>(), 4444);
        assert_eq!(peeked.get_num_ne::<u64>(), 5555);
        assert_eq!(peeked.get_num_ne::<u64>(), 6666);
        assert_eq!(peeked.get_num_ne::<u64>(), 7777);
        assert_eq!(peeked.get_num_ne::<u64>(), 8888);

        for _ in 0..8 {
            assert_eq!(peeked.get_byte(), 9);
        }

        assert_eq!(peeked.len(), 0);
        assert_eq!(peeked.first_slice().len(), 0);

        // Fill up the remaining 28 bytes of data so we have a full sequence builder.
        buf.put_byte_repeated(88, 28);

        let mut peeked = buf.peek();
        peeked.advance(72);

        assert_eq!(peeked.len(), 28);

        for _ in 0..28 {
            assert_eq!(peeked.get_byte(), 88);
        }
    }

    #[test]
    fn consume_part_of_frozen_span() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);

        let memory = FixedBlockMemory::new(nz!(10));

        buf.reserve(100, &memory);

        assert_eq!(buf.capacity(), 100);

        buf.put_num_ne(1111_u64);
        // This freezes the first span of 10, as we filled it all up.
        buf.put_num_ne(2222_u64);

        let mut first8 = buf.consume(U64_SIZE);
        assert_eq!(first8.get_num_ne::<u64>(), 1111);
        assert!(first8.is_empty());

        buf.put_num_ne(3333_u64);

        let mut second16 = buf.consume(16);
        assert_eq!(second16.get_num_ne::<u64>(), 2222);
        assert_eq!(second16.get_num_ne::<u64>(), 3333);
        assert!(second16.is_empty());
    }

    #[test]
    fn empty_buffer() {
        let mut buf = BytesBuf::new();
        assert!(buf.is_empty());
        assert!(buf.peek().is_empty());
        assert_eq!(0, buf.first_unfilled_slice().len());

        let consumed = buf.consume(0);
        assert!(consumed.is_empty());

        let consumed = buf.consume_all();
        assert!(consumed.is_empty());
    }

    #[test]
    fn iter_available_empty_with_capacity() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);

        let memory = FixedBlockMemory::new(nz!(100));

        // Capacity: 0 -> 1000 (10x100)
        buf.reserve(1000, &memory);

        assert_eq!(buf.capacity(), 1000);
        assert_eq!(buf.remaining_capacity(), 1000);

        let iter = buf.iter_available_capacity(None);

        // Demonstrating that we can access slices concurrently, not only one by one.
        let slices: Vec<_> = iter.map(|(s, _)| s).collect();

        assert_eq!(slices.len(), 10);

        for slice in slices {
            assert_eq!(slice.len(), 100);
        }

        // After we have dropped all slice references, it is again legal to access the buffer.
        // This is blocked by the borrow checker while slice references still exist.
        buf.reserve(100, &memory);
    }

    #[test]
    fn iter_available_nonempty() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);

        let memory = FixedBlockMemory::new(nz!(8));

        // Capacity: 0 -> 16 (2x8)
        buf.reserve(TWO_U64_SIZE, &memory);

        assert_eq!(buf.capacity(), 16);
        assert_eq!(buf.remaining_capacity(), 16);

        // We write an u64 - this fills half the capacity and should result in
        // the first span builder being frozen and the second remaining in its entirety.
        buf.put_num_ne(1234_u64);

        assert_eq!(buf.len(), 8);
        assert_eq!(buf.remaining_capacity(), 8);

        let available_slices: Vec<_> = buf.iter_available_capacity(None).map(|(s, _)| s).collect();
        assert_eq!(available_slices.len(), 1);
        assert_eq!(available_slices[0].len(), 8);

        // We write a u32 - this fills half the remaining capacity, which results
        // in a half-filled span builder remaining in the buffer.
        buf.put_num_ne(5678_u32);

        assert_eq!(buf.len(), 12);
        assert_eq!(buf.remaining_capacity(), 4);

        let available_slices: Vec<_> = buf.iter_available_capacity(None).map(|(s, _)| s).collect();
        assert_eq!(available_slices.len(), 1);
        assert_eq!(available_slices[0].len(), 4);

        // We write a final u32 to use up all the capacity.
        buf.put_num_ne(9012_u32);

        assert_eq!(buf.len(), 16);
        assert_eq!(buf.remaining_capacity(), 0);

        assert_eq!(buf.iter_available_capacity(None).count(), 0);
    }

    #[test]
    fn iter_available_empty_no_capacity() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);
        assert_eq!(buf.iter_available_capacity(None).count(), 0);
    }

    #[test]
    fn vectored_write_zero() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);

        let memory = FixedBlockMemory::new(nz!(8));

        // Capacity: 0 -> 16 (2x8)
        buf.reserve(TWO_U64_SIZE, &memory);

        assert_eq!(buf.capacity(), 16);
        assert_eq!(buf.remaining_capacity(), 16);

        let vectored_write = buf.begin_vectored_write(None);

        // SAFETY: Yes, we really wrote 0 bytes.
        unsafe {
            vectored_write.commit(0);
        }

        assert_eq!(buf.capacity(), 16);
        assert_eq!(buf.remaining_capacity(), 16);
    }

    #[test]
    fn vectored_write_one_slice() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);

        let memory = FixedBlockMemory::new(nz!(8));

        // Capacity: 0 -> 8 (1x8)
        buf.reserve(U64_SIZE, &memory);

        assert_eq!(buf.capacity(), 8);
        assert_eq!(buf.remaining_capacity(), 8);

        let mut vectored_write = buf.begin_vectored_write(None);

        let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].len(), 8);

        write_copy_of_slice(slices[0], &0x3333_3333_3333_3333_u64.to_ne_bytes());

        // SAFETY: Yes, we really wrote 8 bytes.
        unsafe {
            vectored_write.commit(8);
        }

        assert_eq!(buf.len(), 8);
        assert_eq!(buf.remaining_capacity(), 0);
        assert_eq!(buf.capacity(), 8);

        let mut result = buf.consume(U64_SIZE);
        assert_eq!(result.get_num_ne::<u64>(), 0x3333_3333_3333_3333);
    }

    #[test]
    fn vectored_write_multiple_slices() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);

        let memory = FixedBlockMemory::new(nz!(8));

        // Capacity: 0 -> 24 (3x8)
        buf.reserve(THREE_U64_SIZE, &memory);

        assert_eq!(buf.capacity(), 24);
        assert_eq!(buf.remaining_capacity(), 24);

        let mut vectored_write = buf.begin_vectored_write(None);

        let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
        assert_eq!(slices.len(), 3);
        assert_eq!(slices[0].len(), 8);
        assert_eq!(slices[1].len(), 8);
        assert_eq!(slices[2].len(), 8);

        // We fill 12 bytes, leaving middle chunk split in half between filled/available.

        write_copy_of_slice(slices[0], &0x3333_3333_3333_3333_u64.to_ne_bytes());
        write_copy_of_slice(slices[1], &0x4444_4444_u32.to_ne_bytes());

        // SAFETY: Yes, we really wrote 12 bytes.
        unsafe {
            vectored_write.commit(12);
        }

        assert_eq!(buf.len(), 12);
        assert_eq!(buf.remaining_capacity(), 12);
        assert_eq!(buf.capacity(), 24);

        let mut vectored_write = buf.begin_vectored_write(None);

        let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].len(), 4);
        assert_eq!(slices[1].len(), 8);

        // We fill the remaining 12 bytes.

        write_copy_of_slice(slices[0], &0x5555_5555_u32.to_ne_bytes());
        write_copy_of_slice(slices[1], &0x6666_6666_6666_6666_u64.to_ne_bytes());

        // SAFETY: Yes, we really wrote 12 bytes.
        unsafe {
            vectored_write.commit(12);
        }

        assert_eq!(buf.len(), 24);
        assert_eq!(buf.remaining_capacity(), 0);
        assert_eq!(buf.capacity(), 24);

        let mut result = buf.consume(THREE_U64_SIZE);
        assert_eq!(result.get_num_ne::<u64>(), 0x3333_3333_3333_3333);
        assert_eq!(result.get_num_ne::<u32>(), 0x4444_4444);
        assert_eq!(result.get_num_ne::<u32>(), 0x5555_5555);
        assert_eq!(result.get_num_ne::<u64>(), 0x6666_6666_6666_6666);
    }

    #[test]
    fn vectored_write_max_len() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);

        let memory = FixedBlockMemory::new(nz!(8));

        // Capacity: 0 -> 24 (3x8)
        buf.reserve(THREE_U64_SIZE, &memory);

        assert_eq!(buf.capacity(), 24);
        assert_eq!(buf.remaining_capacity(), 24);

        // We limit to 13 bytes of visible capacity, of which we will fill 12.
        let mut vectored_write = buf.begin_vectored_write(Some(13));

        let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].len(), 8);
        assert_eq!(slices[1].len(), 5);

        // We fill 12 bytes, leaving middle chunk split in half between filled/available.

        write_copy_of_slice(slices[0], &0x3333_3333_3333_3333_u64.to_ne_bytes());
        write_copy_of_slice(slices[1], &0x4444_4444_u32.to_ne_bytes());

        // SAFETY: Yes, we really wrote 12 bytes.
        unsafe {
            vectored_write.commit(12);
        }

        assert_eq!(buf.len(), 12);
        assert_eq!(buf.remaining_capacity(), 12);
        assert_eq!(buf.capacity(), 24);

        // There are 12 remaining and we set max_limit to exactly cover those 12
        let mut vectored_write = buf.begin_vectored_write(Some(12));

        let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].len(), 4);
        assert_eq!(slices[1].len(), 8);

        write_copy_of_slice(slices[0], &0x5555_5555_u32.to_ne_bytes());
        write_copy_of_slice(slices[1], &0x6666_6666_6666_6666_u64.to_ne_bytes());

        // SAFETY: Yes, we really wrote 12 bytes.
        unsafe {
            vectored_write.commit(12);
        }

        assert_eq!(buf.len(), 24);
        assert_eq!(buf.remaining_capacity(), 0);
        assert_eq!(buf.capacity(), 24);

        let mut result = buf.consume(THREE_U64_SIZE);
        assert_eq!(result.get_num_ne::<u64>(), 0x3333_3333_3333_3333);
        assert_eq!(result.get_num_ne::<u32>(), 0x4444_4444);
        assert_eq!(result.get_num_ne::<u32>(), 0x5555_5555);
        assert_eq!(result.get_num_ne::<u64>(), 0x6666_6666_6666_6666);
    }

    #[test]
    fn vectored_write_max_len_overflow() {
        let mut buf = BytesBuf::new();

        let memory = FixedBlockMemory::new(nz!(8));

        // Capacity: 0 -> 24 (3x8)
        buf.reserve(THREE_U64_SIZE, &memory);

        assert_eq!(buf.capacity(), 24);
        assert_eq!(buf.remaining_capacity(), 24);

        // We ask for 25 bytes of capacity but there are only 24 available. Oops!
        assert_panic!(buf.begin_vectored_write(Some(25)));
    }

    #[test]
    fn vectored_write_overcommit() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);

        let memory = FixedBlockMemory::new(nz!(8));

        // Capacity: 0 -> 16 (2x8)
        buf.reserve(TWO_U64_SIZE, &memory);

        assert_eq!(buf.capacity(), 16);
        assert_eq!(buf.remaining_capacity(), 16);

        let vectored_write = buf.begin_vectored_write(None);

        assert_panic!(
            // SAFETY: Intentionally lying here to trigger a panic.
            unsafe {
                vectored_write.commit(17);
            }
        );
    }

    #[test]
    fn vectored_write_abort() {
        let mut buf = BytesBuf::new();

        assert_eq!(buf.capacity(), 0);
        assert_eq!(buf.remaining_capacity(), 0);

        let memory = FixedBlockMemory::new(nz!(8));

        // Capacity: 0 -> 8 (1x8)
        buf.reserve(U64_SIZE, &memory);

        assert_eq!(buf.capacity(), 8);
        assert_eq!(buf.remaining_capacity(), 8);

        let mut vectored_write = buf.begin_vectored_write(None);

        let mut slices: Vec<_> = vectored_write.slices_mut().map(|(s, _)| s).collect();
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].len(), 8);

        write_copy_of_slice(slices[0], &0x3333_3333_3333_3333_u64.to_ne_bytes());

        // Actually never mind - we drop it here.
        #[expect(clippy::drop_non_drop, reason = "Just being explicit for illustration")]
        drop(vectored_write);

        assert_eq!(buf.len(), 0);
        assert_eq!(buf.remaining_capacity(), 8);
        assert_eq!(buf.capacity(), 8);
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

            let mut buf = BytesBuf::from_blocks([block1, block2]);

            // Freezes first span of 8, retains one span builder.
            buf.put_num_ne(1234_u64);

            assert_eq!(buf.frozen_spans.len(), 1);
            assert_eq!(buf.span_builders_reversed.len(), 1);

            buf.extend_lifetime()
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

            let mut buf = BytesBuf::from_blocks([block1, block2]);

            // Freezes first span of 8, retains one span builder.
            buf.put_num_ne(1234_u64);

            assert_eq!(buf.frozen_spans.len(), 1);
            assert_eq!(buf.span_builders_reversed.len(), 1);

            let vectored_write = buf.begin_vectored_write(None);

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
    fn from_view() {
        let memory = GlobalPool::new();

        let view1 = BytesView::copied_from_slice(b"bla bla bla", &memory);

        let mut buf: BytesBuf = view1.clone().into();

        let view2 = buf.consume_all();

        assert_eq!(view1, view2);
    }

    #[test]
    fn consume_manifest_correctly_calculated() {
        let memory = FixedBlockMemory::new(nz!(10));

        let mut buf = BytesBuf::new();
        buf.reserve(100, &memory);

        // 32 bytes in 3 spans.
        buf.put_num_ne(1111_u64);
        buf.put_num_ne(1111_u64);
        buf.put_num_ne(1111_u64);
        buf.put_num_ne(1111_u64);

        // Freeze it all - a precondition to consuming is to freeze everything first.
        buf.ensure_frozen(32);

        let consume8 = buf.prepare_consume(8);

        assert_eq!(consume8.detach_complete_frozen_spans, 0);
        assert_eq!(consume8.consume_partial_span_bytes, 8);
        assert_eq!(consume8.required_spans_capacity(), 1);

        let consume10 = buf.prepare_consume(10);

        assert_eq!(consume10.detach_complete_frozen_spans, 1);
        assert_eq!(consume10.consume_partial_span_bytes, 0);
        assert_eq!(consume10.required_spans_capacity(), 1);

        let consume11 = buf.prepare_consume(11);

        assert_eq!(consume11.detach_complete_frozen_spans, 1);
        assert_eq!(consume11.consume_partial_span_bytes, 1);
        assert_eq!(consume11.required_spans_capacity(), 2);

        let consume30 = buf.prepare_consume(30);

        assert_eq!(consume30.detach_complete_frozen_spans, 3);
        assert_eq!(consume30.consume_partial_span_bytes, 0);
        assert_eq!(consume30.required_spans_capacity(), 3);

        let consume31 = buf.prepare_consume(31);

        assert_eq!(consume31.detach_complete_frozen_spans, 3);
        assert_eq!(consume31.consume_partial_span_bytes, 1);
        assert_eq!(consume31.required_spans_capacity(), 4);

        let consume32 = buf.prepare_consume(32);

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
    fn peek_empty_builder() {
        let buf = BytesBuf::new();
        let peeked = buf.peek();

        assert!(peeked.is_empty());
        assert_eq!(peeked.len(), 0);
    }

    #[test]
    fn peek_with_frozen_spans_only() {
        let memory = FixedBlockMemory::new(nz!(10));
        let mut buf = BytesBuf::new();

        buf.reserve(20, &memory);
        buf.put_num_ne(0x1111_1111_1111_1111_u64);
        buf.put_num_ne(0x2222_2222_2222_2222_u64);
        // Both blocks are now frozen (filled completely)
        assert_eq!(buf.len(), 16);

        let mut peeked = buf.peek();

        assert_eq!(peeked.len(), 16);
        assert_eq!(peeked.get_num_ne::<u64>(), 0x1111_1111_1111_1111);
        assert_eq!(peeked.get_num_ne::<u64>(), 0x2222_2222_2222_2222);

        // Original builder still has the data
        assert_eq!(buf.len(), 16);
    }

    #[test]
    fn peek_with_partially_filled_span_builder() {
        let memory = FixedBlockMemory::new(nz!(10));
        let mut buf = BytesBuf::new();

        buf.reserve(10, &memory);
        buf.put_num_ne(0x3333_3333_3333_3333_u64);
        buf.put_num_ne(0x4444_u16);
        // We have 10 bytes filled in a 10-byte block
        assert_eq!(buf.len(), 10);

        let mut peeked = buf.peek();

        assert_eq!(peeked.len(), 10);
        assert_eq!(peeked.get_num_ne::<u64>(), 0x3333_3333_3333_3333);
        assert_eq!(peeked.get_num_ne::<u16>(), 0x4444);

        // Original builder still has the data
        assert_eq!(buf.len(), 10);
    }

    #[test]
    fn peek_preserves_capacity_of_partial_span_builder() {
        let memory = FixedBlockMemory::new(nz!(20));
        let mut buf = BytesBuf::new();

        buf.reserve(20, &memory);
        buf.put_num_ne(0x5555_5555_5555_5555_u64);

        // We have 8 bytes filled and 12 bytes remaining capacity
        assert_eq!(buf.len(), 8);
        assert_eq!(buf.remaining_capacity(), 12);

        let mut peeked = buf.peek();

        assert_eq!(peeked.len(), 8);
        assert_eq!(peeked.get_num_ne::<u64>(), 0x5555_5555_5555_5555);

        // CRITICAL TEST: Capacity should be preserved
        assert_eq!(buf.len(), 8);
        assert_eq!(buf.remaining_capacity(), 12);

        // We should still be able to write more data
        buf.put_num_ne(0x6666_6666_u32);
        assert_eq!(buf.len(), 12);
        assert_eq!(buf.remaining_capacity(), 8);

        // And we can peek again to see the updated data
        let mut peeked2 = buf.peek();
        assert_eq!(peeked2.len(), 12);
        assert_eq!(peeked2.get_num_ne::<u64>(), 0x5555_5555_5555_5555);
        assert_eq!(peeked2.get_num_ne::<u32>(), 0x6666_6666);
    }

    #[test]
    fn peek_with_mixed_frozen_and_unfrozen() {
        let memory = FixedBlockMemory::new(nz!(10));
        let mut buf = BytesBuf::new();

        buf.reserve(30, &memory);

        // Fill first block completely (10 bytes) - will be frozen
        buf.put_num_ne(0x1111_1111_1111_1111_u64);
        buf.put_num_ne(0x2222_u16);

        // Fill second block completely (10 bytes) - will be frozen
        buf.put_num_ne(0x3333_3333_3333_3333_u64);
        buf.put_num_ne(0x4444_u16);

        // Partially fill third block (only 4 bytes) - will remain unfrozen
        buf.put_num_ne(0x5555_5555_u32);

        assert_eq!(buf.len(), 24);
        assert_eq!(buf.remaining_capacity(), 6);

        let mut peeked = buf.peek();

        assert_eq!(peeked.len(), 24);
        assert_eq!(peeked.get_num_ne::<u64>(), 0x1111_1111_1111_1111);
        assert_eq!(peeked.get_num_ne::<u16>(), 0x2222);
        assert_eq!(peeked.get_num_ne::<u64>(), 0x3333_3333_3333_3333);
        assert_eq!(peeked.get_num_ne::<u16>(), 0x4444);
        assert_eq!(peeked.get_num_ne::<u32>(), 0x5555_5555);
        // Original builder still has all the data and capacity
        assert_eq!(buf.len(), 24);
        assert_eq!(buf.remaining_capacity(), 6);
    }

    #[test]
    fn peek_then_consume() {
        let memory = FixedBlockMemory::new(nz!(20));
        let mut buf = BytesBuf::new();

        buf.reserve(20, &memory);
        buf.put_num_ne(0x7777_7777_7777_7777_u64);
        buf.put_num_ne(0x8888_8888_u32);
        assert_eq!(buf.len(), 12);

        // Peek at the data
        let mut peeked = buf.peek();
        assert_eq!(peeked.len(), 12);
        assert_eq!(peeked.get_num_ne::<u64>(), 0x7777_7777_7777_7777);

        // Original builder still has the data
        assert_eq!(buf.len(), 12);

        // Now consume some of it
        let mut consumed = buf.consume(8);
        assert_eq!(consumed.get_num_ne::<u64>(), 0x7777_7777_7777_7777);

        // Builder should have less data now
        assert_eq!(buf.len(), 4);

        // Peek again should show the remaining data
        let mut peeked2 = buf.peek();
        assert_eq!(peeked2.len(), 4);
        assert_eq!(peeked2.get_num_ne::<u32>(), 0x8888_8888);
    }

    #[test]
    fn peek_multiple_times() {
        let memory = FixedBlockMemory::new(nz!(20));
        let mut buf = BytesBuf::new();

        buf.reserve(20, &memory);
        buf.put_num_ne(0xAAAA_AAAA_AAAA_AAAA_u64);

        // Peek multiple times - each should work independently
        let mut peeked1 = buf.peek();
        let mut peeked2 = buf.peek();

        assert_eq!(peeked1.get_num_ne::<u64>(), 0xAAAA_AAAA_AAAA_AAAA);
        assert_eq!(peeked2.get_num_ne::<u64>(), 0xAAAA_AAAA_AAAA_AAAA);

        // Original builder still intact
        assert_eq!(buf.len(), 8);
    }

    #[test]
    fn first_unfilled_slice_meta_no_capacity() {
        let buf = BytesBuf::new();
        assert!(buf.first_unfilled_slice_meta().is_none());
    }

    #[test]
    fn first_unfilled_slice_meta_no_meta() {
        let memory = FixedBlockMemory::new(nz!(64));
        let buf = memory.reserve(64);
        assert!(buf.first_unfilled_slice_meta().is_none());
    }

    #[test]
    fn first_unfilled_slice_meta_with_meta() {
        #[derive(Debug)]
        struct CustomMeta;

        impl BlockMeta for CustomMeta {}

        // SAFETY: We are not allowed to drop this until all BlockRef are gone. This is fine
        // because it is dropped at the end of the function, after all BlockRef instances.
        let block = unsafe { TestMemoryBlock::new(nz!(100), Some(Box::new(CustomMeta))) };
        let block = pin!(block);

        // SAFETY: We guarantee exclusive access to the memory capacity.
        let block = unsafe { block.as_ref().to_block() };

        let buf = BytesBuf::from_blocks([block]);
        let meta = buf.first_unfilled_slice_meta().expect("should have metadata");
        assert!(meta.is::<CustomMeta>());
        assert!(!meta.is::<u8>());
    }

    // To be stabilized soon: https://github.com/rust-lang/rust/issues/79995
    fn write_copy_of_slice(dst: &mut [MaybeUninit<u8>], src: &[u8]) {
        assert!(dst.len() >= src.len());

        // SAFETY: We have verified that dst is large enough.
        unsafe {
            src.as_ptr().copy_to_nonoverlapping(dst.as_mut_ptr().cast(), src.len());
        }
    }

    // Compile time test
    fn _can_use_in_dyn_traits(mem: &dyn Memory) {
        let mut buf = mem.reserve(123);
        let _ = buf.writer(mem);
    }
}
