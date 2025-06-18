// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::panic;
use std::collections::VecDeque;
use std::mem::{self, MaybeUninit};
use std::num::{NonZero, NonZeroUsize};
use std::ptr::NonNull;
use std::slice;

use bytes::buf::UninitSlice;
use bytes::{Buf, BufMut};

use crate::{InspectSpanBuilderData, MemoryGuard, ProvideMemory, Sequence, Span, SpanBuilder};

/// Allows you to write into memory owned by the I/O subsystem and to make use of
/// the written data by transforming it into one or more [`Sequence`]s.
///
/// The capacity of the `SequenceBuilder` must be reserved in advance via [`reserve()`][3] before
/// you can fill it with data.
///
/// # Conceptual design
///
/// The memory owned by a `SequenceBuilder` can be viewed as two regions:
///
/// * Filled memory - these bytes have been written to but have not yet been consumed as a
///   [`Sequence`]. They may be inspected (via [`inspect()`][4]) or consumed (via [`consume()`][5]).
/// * Available memory - these bytes have not yet been written to and are available for writing via
///   [`bytes::buf::BufMut`][1] or [`begin_vectored_write()`][Self::begin_vectored_write].
///
/// Existing [`Sequence`]s can be appended to the [`SequenceBuilder`] via [`append()`][6] without
/// consuming capacity (each appended [`Sequence`] brings its own backing memory).
///
#[doc = include_str!("../doc/snippets/sequence_memory_layout.md")]
///
/// [1]: https://docs.rs/bytes/latest/bytes/buf/trait.BufMut.html
/// [3]: Self::reserve
/// [4]: Self::inspect
/// [5]: Self::consume
/// [6]: Self::append
#[derive(Default)]
pub struct SequenceBuilder {
    // The frozen spans are at the front of the sequence being built and have already become
    // immutable (or already arrived in that form). They will be consumed first.
    // TODO: Avoid this inefficient dynamic allocation, at least for typical cases.
    frozen_spans: VecDeque<Span>,

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
    // TODO: Avoid this inefficient dynamic allocation, at least for typical cases.
    span_builders: VecDeque<SpanBuilder>,
}

impl SequenceBuilder {
    /// Crates an instance with 0 bytes of capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn from_span_builders(span_builders: impl IntoIterator<Item = SpanBuilder>) -> Self {
        Self {
            frozen_spans: VecDeque::new(),
            span_builders: span_builders.into_iter().collect(),
        }
    }

    /// Adds memory to the sequence builder, ensuring there is enough capacity to accommodate
    /// `additional_bytes` of content in addition to existing content already present.
    ///
    /// The requested reserve capacity may be exceeded if the memory provider considers it more
    /// efficient to use a larger block of memory than strictly required for this operation.
    pub fn reserve(&mut self, additional_bytes: usize, memory_provider: &impl ProvideMemory) {
        let bytes_needed = additional_bytes.saturating_sub(self.remaining_mut());

        if bytes_needed == 0 {
            return;
        }

        self.extend_capacity_by_at_least(bytes_needed, memory_provider);
    }

    fn extend_capacity_by_at_least(&mut self, bytes: usize, memory_provider: &impl ProvideMemory) {
        let additional_memory = memory_provider.reserve(bytes);

        // For extra paranoia. Maybe remove when stabilizing.
        assert!(additional_memory.capacity() >= bytes);
        assert!(additional_memory.is_empty());

        let capacity_before = self.capacity();

        self.span_builders.extend(additional_memory.span_builders);

        // Throw in a quick sanity check to avoid silly errors during development.
        let capacity_added = self.capacity().saturating_sub(capacity_before);
        assert!(capacity_added >= bytes);
    }

    /// Appends the given sequence to the end of the sequence builder's filled bytes region.
    ///
    /// This automatically extends the builder's capacity with memory from the
    /// appended sequence, for a net zero change in remaining available capacity.
    pub fn append(&mut self, sequence: Sequence) {
        if !sequence.has_remaining() {
            return;
        }

        // Only the first span builder may hold unfrozen data (the rest are for spare capacity).
        let total_unfrozen_bytes =
            NonZero::new(self.span_builders.front().map_or(0, SpanBuilder::len));

        if let Some(total_unfrozen_bytes) = total_unfrozen_bytes {
            // If there is any unfrozen data, we freeze it now to ensure we append after all
            // existing data already in the sequence builder.
            self.freeze_from_first(total_unfrozen_bytes);

            // Debug build paranoia: nothing remains in the span builder, right?
            debug_assert!(self.span_builders.front().map_or(0, SpanBuilder::len) == 0);
        }

        self.frozen_spans.extend(sequence.into_spans());
    }

    /// Inspects the contents of the filled bytes region of the sequence builder.
    /// Typically used to identify whether and which contents may be consumed.
    #[must_use]
    pub fn inspect(&self) -> SequenceBuilderInspector {
        let cursor = if !self.frozen_spans.is_empty() {
            InspectCursor::FrozenSpan {
                span_index: 0,
                offset_in_span: 0,
            }
        } else if let Some(first_builder) = self.span_builders.front() {
            InspectCursor::FirstSpanBuilder {
                inspector: first_builder.inspect(),
            }
        } else {
            InspectCursor::End
        };

        SequenceBuilderInspector {
            builder: self,
            cursor,
            remaining: self.len(),
        }
    }

    /// Length of the filled bytes region, ready to be consumed.
    ///
    /// # Panics
    ///
    /// TODO: Document panics
    pub fn len(&self) -> usize {
        // TODO: Cache this to avoid recalculating it
        let frozen_len = self.frozen_spans.iter().map(|x| x.len()).sum::<usize>();
        let unfrozen_len = self.span_builders.front().map_or(0, SpanBuilder::len);

        frozen_len
            .checked_add(unfrozen_len)
            .expect("usize overflow should be impossible here")
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    ///
    /// # Panics
    ///
    /// TODO: Document panics
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.len()
            .checked_add(self.remaining_mut())
            .expect("usize overflow should be impossible here")
    }

    /// Consumes `len` bytes from the beginning of the filled bytes region,
    /// returning a [`Sequence`] with those bytes.
    ///
    /// # Panics
    ///
    /// Panics if the filled bytes region does not contain at least `len` bytes.
    pub fn consume(&mut self, len: usize) -> Sequence {
        self.consume_checked(len)
            .expect("attempted to consume more bytes than available in builder")
    }

    /// Consumes `len` bytes from the beginning of the filled bytes region,
    /// returning a [`Sequence`] with those bytes.
    ///
    /// Returns `None` if the filled bytes region does not contain at least `len` bytes.
    ///
    /// # Panics
    ///
    /// TODO: Document panics
    pub fn consume_checked(&mut self, mut len: usize) -> Option<Sequence> {
        if len > self.len() {
            return None;
        }

        self.ensure_frozen(len);

        // TODO: Avoid this dynamic allocation.
        let mut spans = VecDeque::new();

        while let Some(nonzero_len) = NonZero::new(len) {
            let take = self.consume_at_most_first_frozen(nonzero_len);

            len = len
                .checked_sub(take.remaining())
                .expect("we somehow took more bytes than we needed");

            spans.push_back(take);
        }

        Some(Sequence::from_spans(spans))
    }

    /// Consumes all filled bytes (if any), returning a [`Sequence`] with those bytes.
    pub fn consume_all(&mut self) -> Sequence {
        self.consume_checked(self.len()).unwrap_or_default()
    }

    /// Consumes at most `len` bytes from the first frozen span. If less data is available in the
    /// first frozen span, only that amount of data is consumed.
    ///
    /// # Panics
    ///
    /// Panics if there are no frozen spans.
    fn consume_at_most_first_frozen(&mut self, len: NonZeroUsize) -> Span {
        let span = self
            .frozen_spans
            .front()
            .expect("attempted to consume from the first frozen span when we had no frozen spans");

        let bytes_in_span = span.remaining();
        debug_assert_ne!(
            bytes_in_span, 0,
            "we somehow ended up with an empty frozen span at the front of the queue"
        );

        let take_bytes = bytes_in_span.min(len.get());

        let take = span.slice(0..take_bytes);
        // This may be empty if we consumed the entire span.
        let keep = span.slice(take_bytes..bytes_in_span);

        self.frozen_spans.pop_front();

        if !keep.is_empty() {
            self.frozen_spans.push_front(keep);
        }

        take
    }

    /// Consumes `len` bytes from the first span builder and moves it to the frozen spans list.
    fn freeze_from_first(&mut self, len: NonZeroUsize) {
        let span_builder = self.span_builders.front_mut().expect(
            "there must be at least one span builder for it to be possible to freeze bytes",
        );

        assert!(len.get() <= span_builder.len());

        let span = span_builder.consume(len);
        self.frozen_spans.push_back(span);

        if span_builder.remaining_mut() == 0 {
            // No more capacity left in this builder, so drop it.
            self.span_builders.pop_front();
        }
    }

    /// Ensures that the frozen spans list contains at least `len` bytes of data, freezing
    /// additional data from the span builders if necessary.
    ///
    /// # Panics
    ///
    /// Panics if there is not enough data in the span builders to fulfill the request.
    fn ensure_frozen(&mut self, len: usize) {
        // TODO: Cache this data to avoid this recalculation.
        let already_frozen_len = self.frozen_spans.iter().map(|x| x.len()).sum::<usize>();
        let must_freeze_bytes = NonZero::new(len.saturating_sub(already_frozen_len));

        let Some(must_freeze_bytes) = must_freeze_bytes else {
            return;
        };

        // We only need to freeze from the first span builder because a type invariant is that
        // only the first span builder may contain data. The others are just spare capacity.
        self.freeze_from_first(must_freeze_bytes);
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
    pub fn begin_vectored_write(&mut self, max_len: Option<usize>) -> SequenceBuilderVectoredWrite {
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
    pub fn begin_vectored_write_checked(
        &mut self,
        max_len: Option<usize>,
    ) -> Option<SequenceBuilderVectoredWrite> {
        if let Some(max_len) = max_len {
            if max_len > self.remaining_mut() {
                return None;
            }
        }

        Some(SequenceBuilderVectoredWrite {
            builder: self,
            max_len,
        })
    }

    fn iter_available_capacity(
        &mut self,
        max_len: Option<usize>,
    ) -> SequenceBuilderAvailableIterator {
        let next_span_builder_index = if self.span_builders.is_empty() {
            None
        } else {
            Some(0)
        };

        SequenceBuilderAvailableIterator {
            builder: self,
            next_span_builder_index,
            max_len,
        }
    }

    /// Creates a memory guard that extends the lifetime of the I/O blocks that provide the backing
    /// memory for this sequence builder.
    ///
    /// This is used to ensure that the I/O blocks are not reused during an I/O operation even if
    /// the originator of the operation drops all `SequenceBuilder` and `SpanBuilder` instances,
    /// making the block unreachable from Rust code.
    fn extend_lifetime(&self) -> MemoryGuard {
        MemoryGuard::new(
            self.span_builders
                .iter()
                .map(SpanBuilder::block)
                .map(Clone::clone)
                .chain(self.frozen_spans.iter().map(Span::block).map(Clone::clone)),
        )
    }
}

// SAFETY: The trait documentation does not define any safety requirements we need to fulfill.
// It is unclear why the trait is marked unsafe in the first place.
unsafe impl BufMut for SequenceBuilder {
    fn remaining_mut(&self) -> usize {
        self.span_builders
            .iter()
            .map(bytes::BufMut::remaining_mut)
            .sum::<usize>()
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        if cnt == 0 {
            return;
        }

        // Advancing the writer by more than a single chunk's length is an error, at least under
        // the current implementation that does not support vectored BufMut access.
        assert!(cnt <= self.chunk_mut().len());

        let span_builder = self
            .span_builders
            .front_mut()
            .expect("there must be at least one span builder if we wrote nonzero bytes");

        // SAFETY: We simply rely on the caller's safety promises here, "forwarding" them.
        unsafe { span_builder.advance_mut(cnt) };

        if span_builder.remaining_mut() == 0 {
            // The span builder is full, so we need to freeze it and move it to the frozen spans.
            let len = NonZero::new(span_builder.len())
                .expect("there is no capacity left in the span builder so there must be at least one byte to consume unless we somehow left an empty span builder in the queue");

            self.freeze_from_first(len);

            // Debug build paranoia: no full span remains after freeze, right?
            debug_assert!(
                self.span_builders
                    .front()
                    .map_or(usize::MAX, BufMut::remaining_mut)
                    > 0
            );
        }
    }

    fn chunk_mut(&mut self) -> &mut bytes::buf::UninitSlice {
        // We are required to always return something, even if we have no span builders!
        self.span_builders.front_mut().map_or_else(
            || {
                // SAFETY: We are responsible for the pointer pointing to a valid storage of the
                // given type (guaranteed by `NonNull::dangling()`) and the rest does not matter
                // because the slice is empty.
                let zero_slice =
                    unsafe { slice::from_raw_parts_mut(NonNull::dangling().as_ptr(), 0) };

                UninitSlice::uninit(zero_slice)
            },
            |x| x.chunk_mut(),
        )
    }
}

impl std::fmt::Debug for SequenceBuilder {
    #[cfg_attr(test, mutants::skip)] // We have no API contract here.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let frozen_spans = self
            .frozen_spans
            .iter()
            .map(|x| x.len().to_string())
            .collect::<Vec<_>>()
            .join(", ");

        let span_builders = self
            .span_builders
            .iter()
            .map(|x| {
                if x.is_empty() {
                    x.remaining_mut().to_string()
                } else {
                    format!("{} + {}", x.len(), x.remaining_mut())
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        f.debug_struct("SequenceBuilder")
            .field("len()", &self.len())
            .field("remaining_mut()", &self.remaining_mut())
            .field("frozen_spans", &frozen_spans)
            .field("span_builders", &span_builders)
            .finish()
    }
}

/// Allows a sequence in the middle of being built to be inspected, typically to identify whether
/// it contains the expected data that allows it (or a part of it) to be consumed.
#[derive(Debug)]
pub struct SequenceBuilderInspector<'b, 'i>
where
    'b: 'i,
{
    builder: &'b SequenceBuilder,
    cursor: InspectCursor<'i>,
    remaining: usize,
}

#[derive(Debug)]
enum InspectCursor<'i> {
    FrozenSpan {
        span_index: usize,
        offset_in_span: usize,
    },
    // Note that only the first span builder can have contents (the rest are spare capacity).
    FirstSpanBuilder {
        inspector: InspectSpanBuilderData<'i>,
    },
    End,
}

impl Buf for SequenceBuilderInspector<'_, '_> {
    fn remaining(&self) -> usize {
        self.remaining
    }

    #[cfg_attr(test, mutants::skip)] // Mutating this can cause infinite loops.
    fn chunk(&self) -> &[u8] {
        match &self.cursor {
            InspectCursor::FrozenSpan {
                span_index,
                offset_in_span,
            } => self
                .builder
                .frozen_spans
                .get(*span_index)
                .expect("cursor referenced a frozen span that does not exist")
                .chunk()
                .get(*offset_in_span..)
                .expect("cursor referenced a frozen span offset that was out of bounds"),
            InspectCursor::FirstSpanBuilder { inspector } => inspector.chunk(),
            InspectCursor::End => &[],
        }
    }

    #[cfg_attr(test, mutants::skip)] // Mutating this can cause infinite loops.
    fn advance(&mut self, mut cnt: usize) {
        if cnt == 0 {
            return;
        }

        while cnt > 0 {
            assert!(cnt <= self.remaining);

            match &mut self.cursor {
                InspectCursor::FrozenSpan {
                    span_index,
                    offset_in_span,
                } => {
                    let span = self
                        .builder
                        .frozen_spans
                        .get(*span_index)
                        .expect("cursor referenced a frozen span that does not exist");

                    let remaining_in_span = span
                        .remaining()
                        .checked_sub(*offset_in_span)
                        .expect("cursor referenced a frozen span offset that was out of bounds");

                    let Some(next_cnt) = cnt.checked_sub(remaining_in_span) else {
                        // We are just advancing within this span but do not go beyond it.
                        *offset_in_span = offset_in_span
                            .checked_add(cnt)
                            .expect("usize overflow is inconceivable here");

                        self.remaining = self.remaining.checked_sub(cnt)
                         .expect("inspection window exceeded inspected content size before running out of content");
                        return;
                    };

                    // We are advancing past the end of this span (and potentially even further).
                    cnt = next_cnt;
                    self.remaining = self.remaining.checked_sub(remaining_in_span)
                        .expect("inspection window exceeded inspected content size before running out of content");

                    // The only question now is what comes next.
                    let next_frozen_span_index = span_index
                        .checked_add(1)
                        .expect("inconceivable to overflow usize here");

                    if self.builder.frozen_spans.len() > next_frozen_span_index {
                        // There is another frozen span for us to process. Do so.
                        *span_index = next_frozen_span_index;
                        *offset_in_span = 0;
                    } else if let Some(first_builder) = self.builder.span_builders.front() {
                        // No more frozen spans but we do have span builders, the first of which
                        // may have some contents to inspect (the rest are only for spare capacity).
                        self.cursor = InspectCursor::FirstSpanBuilder {
                            inspector: first_builder.inspect(),
                        };
                    } else {
                        // No more data?! This is going to be a problem once we loop...
                        self.cursor = InspectCursor::End;
                    }
                }
                InspectCursor::FirstSpanBuilder { inspector } => {
                    // The first span builder is the last thing we can inspect, as any successive
                    // span builders only exist for spare capacity and cannot contain contents.
                    let remaining_in_span_builder = inspector.remaining();

                    assert!(cnt <= remaining_in_span_builder);

                    self.remaining = self.remaining.checked_sub(cnt)
                        .expect("inspection window exceeded inspected content size before running out of content");

                    if cnt == remaining_in_span_builder {
                        self.cursor = InspectCursor::End;
                    } else {
                        inspector.advance(cnt);
                    }

                    // We asserted that this span builder satisfies the command, so we are done.
                    return;
                }
                InspectCursor::End => {
                    panic!("attempted to advance past the end of the inspection window")
                }
            }
        }
    }
}

/// A vectored write is an I/O operation that concurrently writes data into multiple chunks
/// of memory owned by a `SequenceBuilder`.
///
/// The operation takes exclusive ownership of the `SequenceBuilder`. During the vectored write,
/// the remaining capacity of the `SequenceBuilder` is exposed as `MaybeUninit<u8>` slices
/// that at the end of the operation must be filled sequentially and in order, without gaps,
/// in any desired amount (from 0 bytes written to all slices filled).
///
/// The capacity used during the operation can optionally be limited to `max_len` bytes.
///
/// The operation is completed by calling `.commit()` on the instance, after which the instance is
/// consumed and the exclusive ownership of the `SequenceBuilder` released.
///
/// If the type is dropped without committing, the operation is aborted and all remaining capacity
/// is left in a potentially uninitialized state.
#[derive(Debug)]
pub struct SequenceBuilderVectoredWrite<'a> {
    builder: &'a mut SequenceBuilder,
    max_len: Option<usize>,
}

impl SequenceBuilderVectoredWrite<'_> {
    /// Iterates over the chunks of available capacity in the sequence builder,
    /// allowing them to be filled with data.
    pub fn iter_chunks_mut(&mut self) -> SequenceBuilderAvailableIterator {
        self.builder.iter_available_capacity(self.max_len)
    }

    /// Creates a memory guard that extends the lifetime of the I/O blocks that provide the backing
    /// memory for this sequence builder.
    ///
    /// This is used to ensure that the I/O blocks are not reused during an I/O operation even if
    /// the originator of the operation drops all `SequenceBuilder` and `SpanBuilder` instances,
    /// making the block unreachable from Rust code.
    #[must_use]
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
    ///
    /// # Panics
    ///
    /// TODO: Document panics
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
                .span_builders
                .front_mut()
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
pub struct SequenceBuilderAvailableIterator<'a> {
    builder: &'a mut SequenceBuilder,
    next_span_builder_index: Option<usize>,

    // Self-imposed constraint on how much of the available capacity is made visible through
    // this iterator. This can be useful to limit the amount of data that can be written into
    // a `SequenceBuilder` during a vectored write operation without having to limit the
    // actual capacity of the `SequenceBuilder`.
    max_len: Option<usize>,
}

impl<'a> Iterator for SequenceBuilderAvailableIterator<'a> {
    type Item = &'a mut [MaybeUninit<u8>];

    #[cfg_attr(test, mutants::skip)] // This gets mutated into an infinite loop which is not very helpful.
    fn next(&mut self) -> Option<Self::Item> {
        let next_span_builder_index = self.next_span_builder_index?;

        self.next_span_builder_index = Some(
            next_span_builder_index
                .checked_add(1)
                .expect("usize overflow is inconceivable here"),
        );
        if self.next_span_builder_index == Some(self.builder.span_builders.len()) {
            self.next_span_builder_index = None;
        }

        let span_builder = self
            .builder
            .span_builders
            .get_mut(next_span_builder_index)
            .expect("iterator cursor referenced a span builder that does not exist");

        // SAFETY: Must treat it as uninitialized. Yeah, we are, obviously.
        // Somewhat pointless to have the callee be marked unsafe considering
        // it returns a `MaybeUninit` already but okay whatever, we'll play along.
        let uninit_slice_mut = unsafe { span_builder.chunk_mut().as_uninit_slice_mut() };

        // SAFETY: There is nothing Rust can do to promise the reference we return is valid for 'a
        // but we can make such a promise ourselves. In essence, returning the references with 'a
        // this will extend the exclusive ownership of `SequenceBuilder` until all returned chunk
        // references are dropped, even if the iterator itself is dropped earlier. We can do this
        // because we know that to access the chunks requires a reference to the `SequenceBuilder`,
        // so as long as a chunk reference exists, access via the `SequenceBuilder` is blocked.
        // TODO: It would be good to have a (ui) test to verify this.
        let uninit_slice_mut = unsafe {
            mem::transmute::<&mut [MaybeUninit<u8>], &'a mut [MaybeUninit<u8>]>(
                &mut *uninit_slice_mut,
            )
        };

        let uninit_slice_mut = if let Some(max_len) = self.max_len {
            // Limit the visible range of the slice if we have a size limit.
            // If this results in the slice being limited to not its full size,
            // we will also terminate the iteration
            let constrained_len = uninit_slice_mut.len().min(max_len);

            let adjusted_slice = uninit_slice_mut
                .get_mut(..constrained_len)
                .expect("guarded by min() above");

            self.max_len = Some(
                max_len
                    .checked_sub(constrained_len)
                    .expect("guarded by min() above"),
            );

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

#[cfg(test)]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        reason = "Fine in test code, we prefer panic on error"
    )]

    use std::sync::Arc;

    use super::*;
    use crate::DefaultMemoryPool;
    use crate::testing::assert_panic;
    use static_assertions::assert_impl_all;

    const U64_SIZE: usize = size_of::<u64>();
    const TWO_U64_SIZE: usize = size_of::<u64>() + size_of::<u64>();
    const THREE_U64_SIZE: usize = size_of::<u64>() + size_of::<u64>() + size_of::<u64>();

    const POOL_SIZE_1234: NonZeroUsize = NonZeroUsize::new(1234).unwrap();

    #[test]
    fn smoke_test() {
        let pool = DefaultMemoryPool::new(POOL_SIZE_1234);

        let min_length = 1000;

        let mut builder = pool.reserve(min_length);

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
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory_provider = create_memory_provider::<100>();

        // Have 0, desired 10, requesting 10, will get 100.
        builder.reserve(10, &memory_provider);

        assert_eq!(builder.capacity(), 100);
        assert_eq!(builder.remaining_mut(), 100);

        // Write 10 bytes of data just to verify that it does not affect "capacity" logic.
        builder.put_u64(1234);
        builder.put_u16(5678);

        assert_eq!(builder.len(), 10);
        assert_eq!(builder.remaining_mut(), 90);
        assert_eq!(builder.capacity(), 100);

        // Have 100, desired 10+140=150, requesting 50, will get another 100 for a total of 200.
        builder.reserve(140, &memory_provider);

        assert_eq!(builder.len(), 10);
        assert_eq!(builder.remaining_mut(), 190);
        assert_eq!(builder.capacity(), 200);

        // Have 200, desired 10+200=210, 210-200=10, will get another 100.
        builder.reserve(200, &memory_provider);

        assert_eq!(builder.len(), 10);
        assert_eq!(builder.remaining_mut(), 290);
        assert_eq!(builder.capacity(), 300);
    }

    #[test]
    fn append() {
        let pool = DefaultMemoryPool::new(POOL_SIZE_1234);

        let min_length = 1000;

        let mut builder1 = pool.reserve(min_length);
        let mut builder2 = pool.reserve(min_length);

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
        builder2.append(Sequence::default());

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
        let mut builder = SequenceBuilder::new();
        let memory_provider = create_memory_provider::<8>();

        // Reserve some capacity and add initial data.
        builder.reserve(16, &memory_provider);
        builder.put_u64(1111);
        builder.put_u64(2222);

        // Consume some data (the 1111).
        let _ = builder.consume(8);

        // Append a sequence (the 3333).
        let mut append_builder = SequenceBuilder::new();
        append_builder.reserve(8, &memory_provider);
        append_builder.put_u64(3333);
        let sequence = append_builder.consume_all();
        builder.append(sequence);

        // Add more data (the 4444).
        builder.reserve(8, &memory_provider);
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
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory_provider = create_memory_provider::<10>();

        // Inspecting an empty builder is fine, it is just an empty inspector in that case.
        let inspector = builder.inspect();
        assert_eq!(inspector.remaining(), 0);

        builder.reserve(100, &memory_provider);

        assert_eq!(builder.capacity(), 100);

        builder.put_u64(1111);

        // We have 0 frozen spans and 10 span builders,
        // the first of which has 8 bytes of filled content.
        let mut inspector = builder.inspect();
        assert_eq!(inspector.chunk().len(), 8);
        assert_eq!(inspector.get_u64(), 1111);
        assert_eq!(inspector.remaining(), 0);

        builder.put_u64(2222);
        builder.put_u64(3333);
        builder.put_u64(4444);
        builder.put_u64(5555);
        builder.put_u64(6666);
        builder.put_u64(7777);
        builder.put_u64(8888);
        // These will cross a span boundary so we can also observe
        // crossing that boundary during inspection.
        builder.put_bytes(9, 8);

        assert_eq!(builder.len(), 72);
        assert_eq!(builder.capacity(), 100);
        assert_eq!(builder.remaining_mut(), 28);

        // We should have 7 frozen spans and 3 span builders,
        // the first of which has 2 bytes of filled content.
        let mut inspector = builder.inspect();

        assert_eq!(inspector.remaining(), 72);

        // This should be the first frozen span of 10 bytes.
        assert_eq!(inspector.chunk().len(), 10);

        assert_eq!(inspector.get_u64(), 1111);
        assert_eq!(inspector.get_u64(), 2222);

        // We consumed 16 bytes, so should be looking at the remaining 4 bytes in the 2nd span.
        assert_eq!(inspector.chunk().len(), 4);

        assert_eq!(inspector.get_u64(), 3333);
        assert_eq!(inspector.get_u64(), 4444);
        assert_eq!(inspector.get_u64(), 5555);
        assert_eq!(inspector.get_u64(), 6666);
        assert_eq!(inspector.get_u64(), 7777);
        assert_eq!(inspector.get_u64(), 8888);

        for _ in 0..8 {
            assert_eq!(inspector.get_u8(), 9);
        }

        assert_eq!(inspector.remaining(), 0);

        // Reading 0 bytes is always valid.
        inspector.advance(0);

        assert_eq!(inspector.chunk().len(), 0);

        // Fill up the remaining 28 bytes of data so we have a full sequence builder.
        builder.put_bytes(88, 28);

        let mut inspector = builder.inspect();
        inspector.advance(72);

        assert_eq!(inspector.remaining(), 28);

        for _ in 0..28 {
            assert_eq!(inspector.get_u8(), 88);
        }
    }

    #[test]
    fn consume_part_of_frozen_span() {
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory_provider = create_memory_provider::<10>();

        builder.reserve(100, &memory_provider);

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
        let mut builder = SequenceBuilder::new();
        assert!(builder.is_empty());
        assert!(!builder.inspect().has_remaining());
        assert_eq!(0, builder.chunk_mut().len());

        let consumed = builder.consume(0);
        assert!(consumed.is_empty());

        let consumed = builder.consume_all();
        assert!(consumed.is_empty());
    }

    #[test]
    fn thread_safe_type() {
        assert_impl_all!(SequenceBuilder: Send, Sync);
    }

    #[test]
    fn iter_available_empty_with_capacity() {
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory_provider = create_memory_provider::<100>();

        // Capacity: 0 -> 1000 (10x100)
        builder.reserve(1000, &memory_provider);

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
        builder.reserve(100, &memory_provider);
    }

    #[test]
    fn iter_available_nonempty() {
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory_provider = create_memory_provider::<8>();

        // Capacity: 0 -> 16 (2x8)
        builder.reserve(TWO_U64_SIZE, &memory_provider);

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
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);
        assert_eq!(builder.iter_available_capacity(None).count(), 0);
    }

    #[test]
    fn vectored_write_zero() {
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory_provider = create_memory_provider::<8>();

        // Capacity: 0 -> 16 (2x8)
        builder.reserve(TWO_U64_SIZE, &memory_provider);

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
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory_provider = create_memory_provider::<8>();

        // Capacity: 0 -> 8 (1x8)
        builder.reserve(U64_SIZE, &memory_provider);

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
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory_provider = create_memory_provider::<8>();

        // Capacity: 0 -> 24 (3x8)
        builder.reserve(THREE_U64_SIZE, &memory_provider);

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
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory_provider = create_memory_provider::<8>();

        // Capacity: 0 -> 24 (3x8)
        builder.reserve(THREE_U64_SIZE, &memory_provider);

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
        let mut builder = SequenceBuilder::new();

        let memory_provider = create_memory_provider::<8>();

        // Capacity: 0 -> 24 (3x8)
        builder.reserve(THREE_U64_SIZE, &memory_provider);

        assert_eq!(builder.capacity(), 24);
        assert_eq!(builder.remaining_mut(), 24);

        // We ask for 25 bytes of capacity but there are only 24 available. Oops!
        assert_panic!(builder.begin_vectored_write(Some(25)));
    }

    #[test]
    fn vectored_write_overcommit() {
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory_provider = create_memory_provider::<8>();

        // Capacity: 0 -> 16 (2x8)
        builder.reserve(TWO_U64_SIZE, &memory_provider);

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
        let mut builder = SequenceBuilder::new();

        assert_eq!(builder.capacity(), 0);
        assert_eq!(builder.remaining_mut(), 0);

        let memory_provider = create_memory_provider::<8>();

        // Capacity: 0 -> 8 (1x8)
        builder.reserve(U64_SIZE, &memory_provider);

        assert_eq!(builder.capacity(), 8);
        assert_eq!(builder.remaining_mut(), 8);

        let mut vectored_write = builder.begin_vectored_write(None);

        let mut chunks = vectored_write.iter_chunks_mut().collect::<Vec<_>>();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 8);

        chunks[0].put_u64(0x3333_3333_3333_3333);

        // Actually nevermind - we drop it here.
        #[expect(clippy::drop_non_drop, reason = "Just being explicit for illustration")]
        drop(vectored_write);

        assert_eq!(builder.len(), 0);
        assert_eq!(builder.remaining_mut(), 8);
        assert_eq!(builder.capacity(), 8);
    }

    #[test]
    fn extend_lifetime_references_all_blocks() {
        let mut weak_references = Vec::new();

        let guard = {
            let mut builder = SequenceBuilder::new();

            let memory_provider = create_memory_provider::<8>();

            // Capacity: 0 -> 16 (2x8)
            builder.reserve(TWO_U64_SIZE, &memory_provider);

            // Freezes first span, retains one span builder.
            builder.put_u64(1234);

            assert_eq!(builder.frozen_spans.len(), 1);
            assert_eq!(builder.span_builders.len(), 1);

            weak_references.push(Arc::downgrade(builder.frozen_spans[0].block()));
            weak_references.push(Arc::downgrade(builder.span_builders[0].block()));

            builder.extend_lifetime()
        };

        // The guard should keep both weakly referenced blocks alive.
        assert!(weak_references.iter().all(|weak| weak.upgrade().is_some()));

        drop(guard);

        // And now they should all be dead.
        assert!(weak_references.iter().all(|weak| weak.upgrade().is_none()));
    }

    #[test]
    fn extend_lifetime_during_vectored_write_references_all_blocks() {
        let mut weak_references = Vec::new();

        let guard = {
            let mut builder = SequenceBuilder::new();

            let memory_provider = create_memory_provider::<8>();

            // Capacity: 0 -> 16 (2x8)
            builder.reserve(TWO_U64_SIZE, &memory_provider);

            // Freezes first span, retains one span builder.
            builder.put_u64(1234);

            assert_eq!(builder.frozen_spans.len(), 1);
            assert_eq!(builder.span_builders.len(), 1);

            weak_references.push(Arc::downgrade(builder.frozen_spans[0].block()));
            weak_references.push(Arc::downgrade(builder.span_builders[0].block()));

            let vectored_write = builder.begin_vectored_write(None);

            vectored_write.extend_lifetime()
        };

        // The guard should keep both weakly referenced blocks alive.
        assert!(weak_references.iter().all(|weak| weak.upgrade().is_some()));

        drop(guard);

        // And now they should all be dead.
        assert!(weak_references.iter().all(|weak| weak.upgrade().is_none()));
    }

    fn create_memory_provider<const BLOCK_SIZE: usize>() -> impl ProvideMemory {
        DefaultMemoryPool::new(NonZeroUsize::new(BLOCK_SIZE).unwrap())
    }
}