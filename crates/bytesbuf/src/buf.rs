// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::format;
#[cfg(not(test))]
use alloc::string::ToString;
#[cfg(not(test))]
use alloc::vec::Vec;
use core::any::type_name;
use core::mem::{self, MaybeUninit};
use core::num::NonZero;
use core::ptr;

use smallvec::SmallVec;

#[cfg(feature = "std")]
use crate::BytesBufWriter;
use crate::mem::{Block, BlockMeta, BlockSize, Memory};
use crate::telemetry::BufferIdentity;
use crate::{BytesView, MAX_INLINE_SPANS, MemoryGuard, Span, SpanBuilder};

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
#[doc = include_str!("../docs/snippets/sequence_memory_layout.md")]
///
/// # Example
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "std")] {
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
/// buf.put_u16_be(1); // Version
/// buf.put_u32_be(42); // Payload length
/// buf.put_u64_be(0xDEAD_BEEF); // Checksum
///
/// // Consume the buffered data as an immutable BytesView.
/// let message = buf.consume_all();
/// assert_eq!(message.len(), 18);
/// # }
/// # }
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
    identity: BufferIdentity,

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

/// Panics if the true (non-wrapping) sum `len + available` would overflow `usize`.
///
/// `capacity()` returns `len + available`, so that sum must fit in `usize` for `capacity()` not to
/// wrap. The two capacity-growing sites (`reserve` and `append`) each grow only one of `len` or
/// `available`. A bounds check on the grown component alone is insufficient: `len` can grow via
/// appended shared memory while `available` holds separately reserved capacity, so only their sum
/// is bounded.
#[track_caller]
fn assert_capacity_within_bounds(len: usize, available: usize) {
    assert!(
        len.checked_add(available).is_some(),
        "buffer capacity cannot exceed usize::MAX (len={len}, available={available})"
    );
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
        // Blocks represent exclusively owned memory, so their combined capacity is bounded by the
        // virtual address space and can never exceed `usize::MAX` - no overflow check is needed.
        Self::from_span_builders(blocks.into_iter().map(Block::into_span_builder))
    }

    /// Creates a buffer backed by the capacity of a single memory block.
    #[cfg(feature = "std")]
    pub(crate) fn from_block(block: Block) -> Self {
        let span_builder = block.into_span_builder();
        let available = span_builder.remaining_capacity();

        let mut span_builders_reversed = SmallVec::new_const();
        span_builders_reversed.push(span_builder);

        Self {
            identity: BufferIdentity::new(),
            frozen_spans: SmallVec::new_const(),
            span_builders_reversed,
            len: 0,
            frozen: 0,
            available,
        }
    }

    pub(crate) fn from_span_builders<I>(span_builders: I) -> Self
    where
        I: IntoIterator<Item = SpanBuilder>,
    {
        let span_builders: SmallVec<[SpanBuilder; MAX_INLINE_SPANS]> = span_builders.into_iter().collect();

        // No overflow check is needed here: span builders reference exclusively owned memory, so
        // their combined capacity is bounded by the virtual address space and can never exceed
        // `usize::MAX`. This differs from `BytesView`, which references shared (immutable) memory
        // that may be aliased by multiple views and therefore requires an explicit overflow guard.
        let available = span_builders.iter().map(SpanBuilder::remaining_capacity).sum();

        Self {
            identity: BufferIdentity::new(),
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
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
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
    /// buf.put_u32_be(0x1234_5678);
    ///
    /// // Can reserve more capacity at any time.
    /// buf.reserve(100, &memory);
    /// assert!(buf.remaining_capacity() >= 100);
    /// # }
    /// # }
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

        let new_available = self
            .available
            .checked_add(additional_memory.capacity())
            .expect("buffer capacity cannot exceed usize::MAX");

        // Growing `available` must not push the total capacity (`len + available`) past `usize::MAX`.
        // Checking `available` alone is insufficient because `len` can grow independently via
        // appended shared memory (see `append`). This upholds the invariant relied on by `capacity()`.
        assert_capacity_within_bounds(self.len, new_available);

        self.available = new_available;

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

        // Validate capacity before performing any mutation (including the freeze below), so a
        // rejected append leaves the buffer untouched. Freezing changes neither `len` nor
        // `available`, so this check is equivalent whether run before or after it.
        //
        // Appended memory is shared (immutable) and may be aliased by other views, so the logical
        // length can grow independently of physical address-space consumption. We must guard the
        // total capacity (`len + available`), not just `len`, to uphold the invariant `capacity()`
        // relies on.
        let new_len = self.len.checked_add(bytes_len).expect("buffer capacity cannot exceed usize::MAX");
        assert_capacity_within_bounds(new_len, self.available);

        let transferred_identity = if self.is_empty() {
            bytes.take_identity()
        } else {
            self.identity.clear();
            BufferIdentity::empty_transfer()
        };

        // Only the first span builder may hold unfrozen data (the rest are for spare capacity).
        let total_unfrozen_bytes = NonZero::new(self.span_builders_reversed.last().map_or(0, SpanBuilder::len));

        if let Some(total_unfrozen_bytes) = total_unfrozen_bytes {
            // If there is any unfrozen data, we freeze it now to ensure we append after all
            // existing data already in the sequence builder.
            self.freeze_from_first(total_unfrozen_bytes);

            // Debug build paranoia: nothing remains in the span builder, right?
            debug_assert_eq!(self.span_builders_reversed.last().map_or(0, SpanBuilder::len), 0);
        }

        self.len = new_len;

        // Any appended BytesView is frozen by definition, as contents of a BytesView are immutable.
        // This cannot wrap because we verified `len` is in-bounds and `frozen <= len` is a type invariant.
        self.frozen = self.frozen.wrapping_add(bytes_len);

        self.frozen_spans.extend(bytes.into_spans_reversed().into_iter().rev());
        self.identity.replace(transferred_identity);
    }

    /// Tries to write a small byte slice into the buffer by fusing the common case where it fits
    /// entirely within the first unfilled slice of capacity into a single span-builder lookup.
    ///
    /// Returns `true` when the fast path handled the write. Returns `false` without modifying the
    /// buffer when the data does not fit in the first unfilled slice (or there is no builder yet),
    /// in which case the caller must fall back to [`put_slice_looped()`].
    ///
    /// [`put_slice_looped()`]: Self::put_slice_looped
    ///
    /// # Panics
    ///
    /// Panics if there is insufficient remaining capacity in the buffer.
    pub(crate) fn put_small(&mut self, src: &[u8]) -> bool {
        assert!(self.remaining_capacity() >= src.len());

        if let Some(builder) = self.span_builders_reversed.last_mut() {
            let dst = builder.unfilled_slice_mut();

            if dst.len() >= src.len() {
                // SAFETY: both are byte regions and `dst` has at least `src.len()` bytes of room.
                unsafe {
                    ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr().cast(), src.len());
                }

                // SAFETY: we just initialized `src.len()` bytes at the front of the unfilled region.
                unsafe {
                    builder.advance(src.len());
                }

                let freeze_len = (builder.remaining_capacity() == 0)
                    .then(|| NonZero::new(builder.len()).expect("a builder with no remaining capacity must hold at least one filled byte"));

                self.len = self
                    .len
                    .checked_add(src.len())
                    .expect("usize overflow is impossible because buffer capacity is bounded by virtual memory size");

                self.available = self
                    .available
                    .checked_sub(src.len())
                    .expect("guarded by the remaining_capacity() assertion above");

                if let Some(len) = freeze_len {
                    // The builder filled up exactly, so freeze it into a span like the general path.
                    self.freeze_from_first(len);
                }

                return true;
            }
        }

        // The value straddles a capacity boundary (or there is no builder yet); signal the caller
        // to use the general path.
        false
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
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(16);
    /// buf.put_u16_be(0x1234);
    /// buf.put_u16_be(0x5678);
    ///
    /// // Peek at the data without consuming it.
    /// let mut peeked = buf.peek();
    /// assert_eq!(peeked.get_u16_be(), 0x1234);
    /// assert_eq!(peeked.get_u16_be(), 0x5678);
    ///
    /// // Despite consuming from peeked, the buffer still contains all data.
    /// assert_eq!(buf.len(), 4);
    ///
    /// let consumed = buf.consume_all();
    /// assert_eq!(consumed.len(), 4);
    /// # }
    /// # }
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
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(32);
    /// assert_eq!(buf.len(), 0);
    ///
    /// buf.put_u32_be(0x1234_5678);
    /// assert_eq!(buf.len(), 4);
    ///
    /// buf.put_slice(*b"Hello");
    /// assert_eq!(buf.len(), 9);
    ///
    /// _ = buf.consume(4);
    /// assert_eq!(buf.len(), 5);
    /// # }
    /// # }
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

        // Will not overflow: `len` is a component of the total capacity `len + available`, which
        // never overflows `usize` (enforced at the growth sites).
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
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
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
    /// # }
    /// # }
    /// ```
    #[must_use]
    pub fn capacity(&self) -> usize {
        // Will not overflow: `len + available` never overflows `usize` - the growth sites enforce
        // this via `assert_capacity_within_bounds`.
        self.len().wrapping_add(self.remaining_capacity())
    }

    /// How many more bytes can be written into the buffer
    /// before its memory capacity is exhausted.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
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
    /// # }
    /// # }
    /// ```
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
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(32);
    ///
    /// buf.put_u16_be(0x1111);
    /// buf.put_u16_be(0x2222);
    ///
    /// // Consume first part.
    /// let mut first = buf.consume(2);
    /// assert_eq!(first.get_u16_be(), 0x1111);
    ///
    /// // Write more data.
    /// buf.put_u16_be(0x3333);
    ///
    /// // Consume remaining data.
    /// let mut rest = buf.consume(4);
    /// assert_eq!(rest.get_u16_be(), 0x2222);
    /// assert_eq!(rest.get_u16_be(), 0x3333);
    /// # }
    /// # }
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

        let result = BytesView::from_spans_reversed(result_spans_reversed);
        if len != 0 {
            result.replace_identity(self.identity.take());
        }
        Some(result)
    }

    fn prepare_consume(&self, mut len: usize) -> ConsumeManifest {
        debug_assert!(len <= self.frozen);

        let mut detach_complete_frozen_spans: usize = 0;

        for span in &self.frozen_spans {
            let span_len = span.len();

            if span_len as usize <= len {
                // Will not wrap: `len + available` never overflows `usize`, so the number of
                // spans (each holding at least one byte) certainly cannot be a greater number.
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
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(32);
    /// buf.put_slice(*b"Hello, ");
    /// buf.put_slice(*b"world!");
    /// buf.put_u16_be(0x2121); // "!!"
    ///
    /// let message = buf.consume_all();
    ///
    /// assert_eq!(message, b"Hello, world!!!");
    /// assert!(buf.is_empty());
    /// # }
    /// # }
    /// ```
    pub fn consume_all(&mut self) -> BytesView {
        // Delegating to the general consume path is intentional. A dedicated all-at-once path that
        // skips the per-span consume bookkeeping lowers the simulated instruction count but yields
        // no wall-clock improvement: the cost here is dominated by transferring span ownership
        // (BlockRef refcount atomics), which both paths pay equally. Keep this thin.
        // SAFETY: Consuming len() bytes from self cannot possibly be out of bounds.
        unsafe { self.consume_checked(self.len()).unwrap_unchecked() }
    }

    /// Splits off `count` bytes of remaining capacity from the buffer.
    ///
    /// Returns a new `BytesBuf` that owns that capacity and has no contents.
    ///
    /// The buffer's filled data (length) is not affected.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use bytesbuf::mem::Memory;
    ///
    /// let mut buf = memory.reserve(64);
    ///
    /// buf.put_u16_be(0xDEAD);
    ///
    /// let remaining_before = buf.remaining_capacity();
    /// let mut split = buf.split_off_remaining(20);
    ///
    /// // The split-off buffer has the requested capacity and no data.
    /// assert!(split.remaining_capacity() >= 20);
    /// assert!(split.is_empty());
    ///
    /// // The original buffer lost that capacity but kept its data.
    /// assert_eq!(buf.remaining_capacity(), remaining_before - 20);
    /// assert_eq!(buf.len(), 2);
    ///
    /// // The split-off buffer can be used independently.
    /// split.put_u16_be(0xBEEF);
    /// assert_eq!(split.len(), 2);
    /// # }
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `count` is greater than the [`remaining_capacity()`] of the buffer.
    ///
    /// [`remaining_capacity()`]: Self::remaining_capacity
    #[must_use]
    pub fn split_off_remaining(&mut self, count: usize) -> Self {
        self.split_off_remaining_checked(count)
            .expect("attempted to split off more remaining capacity than available in buffer")
    }

    /// Splits off `count` bytes of remaining capacity from the buffer.
    ///
    /// Returns a new `BytesBuf` that owns that capacity and has no contents, or `None`
    /// if `count` is greater than the [`remaining_capacity()`] of the buffer.
    ///
    /// The buffer's filled data (length) is not affected.
    ///
    /// [`remaining_capacity()`]: Self::remaining_capacity
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "only unreachable panics")]
    pub fn split_off_remaining_checked(&mut self, count: usize) -> Option<Self> {
        if count > self.remaining_capacity() {
            return None;
        }

        if count == 0 {
            return Some(Self::new());
        }

        let mut result_builders: SmallVec<[SpanBuilder; MAX_INLINE_SPANS]> = SmallVec::new();
        let mut remaining = count;

        // Walk span_builders_reversed from front (logically last builders, which are empty
        // spare-capacity builders per type invariant). Drain as many whole builders as feasible first.
        let mut whole_span_builders_to_take: usize = 0;

        for span_builder in &self.span_builders_reversed {
            if !span_builder.is_empty() {
                // We have reached the last span builder (logically first) because it is the
                // only one allowed to have data in it. As it is not empty, we exit the loop
                // and will later split this span builder to get any remaining bytes.
                break;
            }

            let capacity_in_span_builder = span_builder.remaining_capacity();

            if capacity_in_span_builder > remaining {
                // This span builder is too big - we have to split it, cannot take entirely.
                break;
            }

            // Will not wrap - guarded by if-statement above.
            remaining = remaining.wrapping_sub(capacity_in_span_builder);
            // Will not wrap as that would imply that there are more span builders than there is virtual memory.
            whole_span_builders_to_take = whole_span_builders_to_take.wrapping_add(1);
        }

        // Drain whole builders from the front (logically last builders).
        result_builders.extend(self.span_builders_reversed.drain(..whole_span_builders_to_take));

        // If we still need more capacity, split the next builder's available capacity.
        if remaining > 0 {
            let span_builder = self.span_builders_reversed.first_mut().expect(
                "remaining_capacity() check at the top ensures a builder is available because we have not received enough capacity yet",
            );

            let remaining: u32 = remaining.try_into()
                .expect("the span builder drain loop ensures that remaining capacity comes from one memory block yet the value is too big to fit into a memory block - impossible");
            let split_count = NonZero::new(remaining).expect("guarded by if-statement above");

            result_builders.push(span_builder.split_off_available(split_count));
        }

        self.available = self.available.checked_sub(count).expect("guarded by bounds check above");

        Some(Self::from_span_builders(result_builders))
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
    #[doc = include_str!("../docs/snippets/sequence_memory_layout.md")]
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
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
    /// # }
    /// # }
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
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
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
    /// # }
    /// # }
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
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
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
    /// # }
    /// # }
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
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
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
    /// # }
    /// # }
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

    /// Converts this instance into a [`Write`][std::io::Write] adapter.
    ///
    /// The memory capacity of the `BytesBuf` will be automatically extended on demand
    /// with additional capacity from the supplied memory provider.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// # (|| {
    /// # let memory = bytesbuf::mem::GlobalPool::new();
    /// use std::io::Write;
    ///
    /// use bytesbuf::mem::Memory;
    ///
    /// let buf = memory.reserve(32);
    /// let mut writer = buf.into_writer(&memory);
    /// writer.write_all(b"Hello, ")?;
    /// writer.write_all(b"world!")?;
    /// let mut buf = writer.into_inner();
    ///
    /// assert_eq!(buf.consume_all(), b"Hello, world!");
    /// # Ok::<(), std::io::Error>(())
    /// # })().unwrap();
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "std")]
    #[cfg_attr(docsrs, doc(cfg(feature = "std")))]
    #[inline]
    pub fn into_writer<M: Memory>(self, memory: M) -> BytesBufWriter<M> {
        BytesBufWriter::new(self, memory)
    }
}

#[expect(
    clippy::missing_fields_in_debug,
    reason = "telemetry identity is intentionally excluded from logical buffer formatting"
)]
impl core::fmt::Debug for BytesBuf {
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

        let mut debug = f.debug_struct(type_name::<Self>());
        debug
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
            // This will not wrap: `len + available` never overflows `usize`, so the count of
            // spans certainly cannot be a greater number.
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

    fn next(&mut self) -> Option<Self::Item> {
        let next_span_builder_index = self.next_span_builder_index?;

        self.next_span_builder_index = Some(
            // Will not overflow: `len + available` never overflows `usize`, so the count of span
            // builders certainly cannot be greater.
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
            // Will not overflow: `len + available` never overflows `usize`, so the count of span
            // builders certainly cannot be greater.
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

#[cfg(feature = "seismograph")]
impl seismograph_io::Buffer for BytesBuf {
    fn recording_state(&self) -> seismograph_io::BufferState {
        seismograph_io::BufferState::new(
            Some(self.identity.get_or_allocate()),
            self.len(),
            self.frozen_spans.len() + usize::from(self.span_builders_reversed.last().is_some_and(|span| !span.is_empty())),
        )
    }
}

#[cfg(test)]
mod tests {
    // These assertions require private spans, consume manifests, identity size, or synthetic overflow inputs.
    #![allow(clippy::indexing_slicing, reason = "Fine in test code, we prefer panic on error")]

    use std::pin::pin;

    use new_zealand::nz;
    use testing_aids::assert_panic;

    use super::*;
    use crate::mem::testing::{FixedBlockMemory, TestMemoryBlock};

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
            buf.put_u64_ne(1234);

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
            buf.put_u64_ne(1234);

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
    fn consume_manifest_correctly_calculated() {
        let memory = FixedBlockMemory::new(nz!(10));

        let mut buf = BytesBuf::new();
        buf.reserve(100, &memory);

        // 32 bytes in 3 spans.
        buf.put_u64_ne(1111);
        buf.put_u64_ne(1111);
        buf.put_u64_ne(1111);
        buf.put_u64_ne(1111);

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
        assert_eq!(size_of::<BytesBuf>(), 552 + size_of::<BufferIdentity>());
    }

    #[test]
    fn capacity_bounds_check_accepts_maximum_total() {
        // The largest representable total capacity is permitted; only exceeding it is rejected.
        assert_capacity_within_bounds(usize::MAX, 0);
        assert_capacity_within_bounds(0, usize::MAX);
        assert_capacity_within_bounds(usize::MAX - 1, 1);
        assert_capacity_within_bounds(usize::MAX / 2, usize::MAX / 2);
    }

    #[test]
    fn capacity_bounds_check_rejects_len_plus_available_overflow() {
        // Neither component overflows on its own, but their sum exceeds usize::MAX. This is the
        // situation that arises when `len` grows via appended shared memory (in `append`) while
        // `available` holds separately reserved capacity (in `extend_capacity_by_at_least`): each
        // site guards only the component it grows, so only this combined check catches the overflow.
        assert_panic!(assert_capacity_within_bounds(usize::MAX, 1));
        assert_panic!(assert_capacity_within_bounds(usize::MAX - 4, 8));
        assert_panic!(assert_capacity_within_bounds(usize::MAX / 2 + 1, usize::MAX / 2 + 1));
    }
}
