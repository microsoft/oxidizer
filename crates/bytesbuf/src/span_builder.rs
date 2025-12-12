// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::slice;
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::ptr::NonNull;

use bytes::buf::UninitSlice;
use bytes::BufMut;

use crate::{BlockRef, BlockSize, Span};

#[cfg(test)]
use std::marker::PhantomData;
#[cfg(test)]
use std::ops::Deref;
#[cfg(test)]
use bytes::Buf;

/// Owns a mutable span of memory capacity from a memory block, which can be filled with data,
/// enabling you to detach spans of immutable bytes from the front to create views over the data.
///
/// Use the [`bytes::buf::BufMut`][1] implementation to fill available memory capacity with data,
/// after which you may detach spans of immutable data from the front via [`consume()`][3].
///
/// Filled bytes may be inspected via [`inspect()`][4] to enable a content-based determination
/// to be made on whether (part of) the filled data is ready to be consumed.
///
/// # Ownership of memory blocks
///
/// When in use, the contents of a memory block are:
///
/// * Zero or more bytes of immutable data.
/// * Zero or more bytes of mutable memory.
///
/// One block may be split into any number of parts, each consisting of either immutable data
/// or mutable memory.
///
/// These parts are always accessed via either:
///
/// 1. Any number of `Spans` over any number of parts consisting of immutable data.
/// 1. At most one `SpanBuilder` over at most one part consisting of mutable memory, which
///    may be partly partly or fully uninitialized.
///
/// When a memory block is first put into use, one `SpanBuilder` is created and has exclusive
/// ownership of the block. From this builder, callers may detach `Span`s from the front to create
/// sub-slices over immutable data of a desired length. The `Span`s may later be cloned/sliced
/// without constraints. The `SpanBuilder` retains exclusive ownership of the remaining part
/// of the memory block (the part that has not been detached as a `Span`).
///
/// Note: `Span` and `SpanBuilder` are private APIs and not exposed in the public API
/// surface. The public API only works with `BytesView` and `BytesBuf`.
///
/// Memory blocks are reference counted to avoid lifetime parameter pollution and provide
/// flexibility in usage. This also implies that we are not using the Rust borrow checker to
/// enforce exclusive reference semantics. Instead, we rely on the guarantees provided by the
/// Span/SpanBuilder types to ensure no forbidden mode of access takes place. This is supported
/// by the following guarantees:
///
/// 1. The only way to write to a memory block is to own a [`SpanBuilder`] that can be used
///    to append data to the block on the fly via `bytes::BufMut` or to read into the block
///    via elementary I/O operations issued to the operating system (typically via participating
///    in a [`BytesBuf`][crate::BytesBuf] vectored read that fills multiple memory blocks simultaneously).
/// 2. Reading from a memory block is only possible once the block (or a slice of it) has been
///    filled with data and the filled region separated into a [`Span`], detaching it from
///    the [`SpanBuilder`]. At this point further mutation of the detached slice is impossible.
///
/// All of this applies to individual slices of memory blocks. That is, when the first part of a
/// memory block is filled with data and detached from [`SpanBuilder`] as an [`Span`], that part
/// becomes immutable but the remainder of the memory block may still contain writable capacity.
///
/// Phrasing this in terms of (imaginary) reference ownership semantics:
///
/// * At most one [`SpanBuilder`] has a `&mut [MaybeUninit<u8>]` to the part of the memory
///   block that has not yet been filled with data.
/// * At most one [`SpanBuilder`] has a `&[u8]` to the part of the memory block that has already been
///   filled with data (or a sub-slice of it, if parts have been detached from the front).
/// * Any number of [`Span`]s have a `&[u8]` to the part of the memory block that has already
///   been filled with data (or sub-slices of it, potentially different sub-slices each).
///
/// In each of these cases, the ownership semantics are mediated via [`Span`] and
/// [`SpanBuilder`] instances that perform the bookkeeping necessary to implement the
/// ownership model. The memory block object itself is ignorant of all this machinery, merely being
/// a reference counting structure around the pointer and length that designates the capacity, with
/// one `BlockRef` indicating one reference. Once all `BlockRef` are dropped, the memory provider
/// may reuse the memory block.
///
/// [1]: https://docs.rs/bytes/latest/bytes/buf/trait.BufMut.html
/// [3]: Self::consume
/// [4]: Self::inspect
#[derive(Debug)]
pub(crate) struct SpanBuilder {
    block_ref: BlockRef,

    // Pointer to the start of the span builder's capacity. This region includes both the memory
    // filled with data as well as the memory that remains available to receive data.
    //
    // Any bytes that have been consumed from the span builder are no longer accessible through
    // this pointer - they are not considered part of the builder's capacity and instead become
    // part of a detached span's capacity.
    start: NonNull<MaybeUninit<u8>>,

    // Number of bytes after `start` that have been filled with data.
    // Any bytes after this range must be treated as uninitialized.
    filled_bytes: BlockSize,

    // Number of bytes after `start + filled_bytes` that may be filled with data.
    // This range of bytes must be treated as uninitialized.
    available_bytes: BlockSize,
}

impl SpanBuilder {
    /// Creates a span builder and gives it exclusive ownership of a memory block.
    ///
    /// The `block_ref` acts as a reference counted handle to the memory block. It may be cloned
    /// at any time to share ownership of the memory block with `Span` instances created by
    /// the `SpanBuilder`. When the last instance from this family of clones is dropped, the
    /// memory capacity associated with the memory block may be released by the memory provider
    /// it originates from.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the `SpanBuilder` being created has exclusive ownership
    /// of the provided memory blocks (i.e. no `BlockRef` clones referencing the same block exist).
    pub(crate) const unsafe fn new(start: NonNull<MaybeUninit<u8>>, len: NonZero<BlockSize>, block_ref: BlockRef) -> Self {
        Self {
            block_ref,
            start,
            filled_bytes: 0,
            available_bytes: len.get(),
        }
    }

    /// Number of bytes at the front that have been filled with data.
    pub(crate) const fn len(&self) -> BlockSize {
        self.filled_bytes
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.filled_bytes == 0
    }

    /// Consumes the specified number of bytes (of already filled data) from the front of the
    /// builder's memory block, returning a span with those immutable bytes.
    ///
    /// # Panics
    ///
    /// Panics if the requested number of bytes to return exceeds the number of bytes filled
    /// with data.
    pub(crate) fn consume(&mut self, len: NonZero<BlockSize>) -> Span {
        self.consume_checked(len)
            .expect("attempted to consume more bytes than available in builder")
    }

    /// Consumes the specified number of bytes (of already filled data) from the front of the
    /// builder's memory block, returning a span with those immutable bytes.
    ///
    /// Returns `None` if the requested number of bytes to return
    /// exceeds the number of bytes filled with data.
    pub(crate) fn consume_checked(&mut self, len: NonZero<BlockSize>) -> Option<Span> {
        if len.get() > self.filled_bytes {
            return None;
        }

        // SAFETY: We must guarantee that the region has been initialized.
        // Yes, it has - this is guarded by the `filled_bytes` check above.
        let span = unsafe { Span::new(self.start.cast(), len.get(), self.block_ref.clone()) };

        // Do this before moving the pointer, so even if something panicks we do not allow
        // out of bounds access via the pointer.
        self.filled_bytes = self
            .filled_bytes
            .checked_sub(len.get())
            .expect("already handled the case where len > filled_bytes");

        // SAFETY: We only seeked over filled bytes, so we must still be in-bounds.
        self.start = unsafe { self.start.add(len.get() as usize) };

        Some(span)
    }

    /// Creates a span over the filled data without consuming it from the builder.
    /// The span shares the BlockRef with the span builder, keeping the memory alive.
    ///
    /// Returns `None` if there is no filled data in the builder.
    ///
    /// # Performance
    ///
    /// This operation creates a new span that shares the same memory block. The operation is
    /// relatively cheap (just cloning a BlockRef and creating a span struct) but should not
    /// be called in tight loops if avoidable.
    pub(crate) fn peek_filled(&self) -> Option<Span> {
        if self.filled_bytes == 0 {
            return None;
        }

        // SAFETY: The data in the span builder up to `filled_bytes` is initialized.
        // The span will share the BlockRef with the span builder, keeping the memory alive.
        Some(unsafe { Span::new(self.start.cast(), self.filled_bytes, self.block_ref.clone()) })
    }

    #[cfg(test)]
    pub(crate) const fn inspect(&self) -> InspectSpanBuilderData<'_> {
        InspectSpanBuilderData {
            start: self.start.cast(),
            len: self.filled_bytes,

            // Borrows the SpanBuilder for the duration of the inspection.
            _builder: PhantomData,
        }
    }

    /// Allows the underlying memory block to be accessed, primarily used to extend its lifetime
    /// beyond that of the `SpanBuilder` itself.
    pub(crate) const fn block(&self) -> &BlockRef {
        &self.block_ref
    }
}

// SAFETY: The trait does not clearly state any safety requirements we must satisfy, so it is
// unclear why this trait is marked unsafe. Cross your fingers and hope for the best!
unsafe impl BufMut for SpanBuilder {
    #[cfg_attr(test, mutants::skip)] // Lying about remaining capacity is a great way to infinite loop.
    fn remaining_mut(&self) -> usize {
        self.available_bytes as usize
    }

    unsafe fn advance_mut(&mut self, cnt: usize) {
        let count = BlockSize::try_from(cnt).expect("attempted to advance past end of span builder");

        // Decrease the end first, so even if there is a panic we do not allow out of bounds access.
        self.available_bytes = self
            .available_bytes
            .checked_sub(count)
            .expect("attempted to advance past end of span builder");

        self.filled_bytes = self
            .filled_bytes
            .checked_add(count)
            .expect("attempted to advance past end of span builder");
    }

    fn chunk_mut(&mut self) -> &mut bytes::buf::UninitSlice {
        // SAFETY: We are seeking past initialized memory, so at most we are at the end of our
        // memory block (which is still valid) but cannot exceed it in any way.
        let available_start = unsafe { self.start.add(self.filled_bytes as usize) };

        // SAFETY: We are responsible for the pointer pointing to a valid storage of the given type
        // (guaranteed by memory block) and for the slice having exclusive access to the memory for
        // the duration of its lifetime (guaranteed by `&mut self` which inherits exclusive access
        // from the SpanBuilder itself).
        let available_slice = unsafe { slice::from_raw_parts_mut(available_start.as_ptr(), self.available_bytes as usize) };

        UninitSlice::uninit(available_slice)
    }
}

#[cfg(test)]
#[derive(Debug)]
pub struct InspectSpanBuilderData<'a> {
    start: NonNull<u8>,
    len: BlockSize,

    // This is just to ensure we borrow the span builder, so it does not get modified concurrently.
    _builder: PhantomData<&'a SpanBuilder>,
}

#[cfg(test)]
impl Deref for InspectSpanBuilderData<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        // SAFETY: We are responsible for the pointer pointing to a valid storage of the given type
        // (guaranteed by memory block) and for there not being any mutation of the memory for the
        // duration of the slice's lifetime (guaranteed by borrowing from the SpanBuilder).
        unsafe { slice::from_raw_parts(self.start.as_ptr(), self.len as usize) }
    }
}

#[cfg(test)]
impl Buf for InspectSpanBuilderData<'_> {
    fn remaining(&self) -> usize {
        self.len as usize
    }

    #[cfg_attr(test, mutants::skip)] // Mutations can cause infinite loops in tests.
    fn chunk(&self) -> &[u8] {
        self
    }

    fn advance(&mut self, cnt: usize) {
        let count = BlockSize::try_from(cnt).expect("attempted to advance inspection window past end of span builder");

        // Length before pointer, so even if we panic below we continue to point in-range.
        self.len = self
            .len
            .checked_sub(count)
            .expect("attempted to advance inspection window past end of span builder");

        // SAFETY: Guaranteed to be in-range via `len` subtraction guard above.
        self.start = unsafe { self.start.add(count as usize) };
    }
}

// SAFETY: The presence of pointers disables Send but we re-enable it here because all our internal
// state is thread-mobile.
unsafe impl Send for SpanBuilder {}
// SAFETY: The presence of pointers disables Sync but we re-enable it here because all our internal
// state is thread-safe (though only for reads - we still require outer mutability).
unsafe impl Sync for SpanBuilder {}

#[cfg(test)]
mod tests {
    use new_zealand::nz;
    use static_assertions::assert_impl_all;
    use testing_aids::assert_panic;

    use super::*;
    use crate::std_alloc_block;

    #[test]
    fn smoke_test() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        assert_eq!(builder.len(), 0);
        assert!(builder.is_empty());
        assert_eq!(builder.remaining_mut(), 10);

        assert!(builder.consume_checked(nz!(1)).is_none());

        builder.put_u64(1234);

        assert_eq!(builder.len(), 8);
        assert!(!builder.is_empty());
        assert_eq!(builder.remaining_mut(), 2);

        _ = builder.consume(nz!(8));

        assert_eq!(builder.len(), 0);
        assert!(builder.is_empty());
        assert_eq!(builder.remaining_mut(), 2);

        builder.put_u16(1234);

        assert_eq!(builder.len(), 2);
        assert!(!builder.is_empty());
        assert_eq!(builder.remaining_mut(), 0);

        _ = builder.consume(nz!(2));

        assert_eq!(builder.len(), 0);
        assert!(builder.is_empty());
        assert_eq!(builder.remaining_mut(), 0);
    }

    #[test]
    fn inspect() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        builder.put_u32(1234);
        builder.put_u32(5678);
        builder.put_u16(90);

        assert_eq!(builder.len(), 10);
        assert_eq!(builder.remaining_mut(), 0);

        let mut inspector = builder.inspect();

        assert_eq!(inspector.remaining(), 10);
        assert_eq!(inspector.chunk().len(), 10);

        assert_eq!(inspector.get_u32(), 1234);
        assert_eq!(inspector.get_u32(), 5678);
        assert_eq!(inspector.get_u16(), 90);

        assert_eq!(inspector.remaining(), 0);
        assert_eq!(inspector.chunk().len(), 0);

        assert_eq!(builder.len(), 10);
        assert_eq!(builder.remaining_mut(), 0);

        _ = builder.consume(nz!(10));

        assert_eq!(builder.len(), 0);
        assert_eq!(builder.remaining_mut(), 0);
    }

    #[test]
    fn append_oob_panics() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        builder.put_u32(1234);
        builder.put_u32(5678);
        assert_panic!(builder.put_u32(90)); // Tries to append 4 when only 2 bytes available.
    }

    #[test]
    fn inspect_oob_panics() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        builder.put_u32(1234);
        builder.put_u32(5678);
        builder.put_u16(90);

        let mut inspector = builder.inspect();
        assert_eq!(inspector.get_u32(), 1234);
        assert_eq!(inspector.get_u32(), 5678);
        assert_panic!(_ = inspector.get_u32()); // Tries to read 4 when only 2 bytes remaining.
    }

    #[test]
    fn thread_safe_type() {
        // The type is thread-mobile (Send) and can be shared (for reads) between threads (Sync).
        assert_impl_all!(SpanBuilder: Send, Sync);
    }

    #[test]
    fn size_change_detector() {
        // The point of this is not to say that we expect it to have a specific size but to allow
        // us to easily detect when the size changes and (if we choose to) bless the change.
        // We assume 64-bit pointers - any support for 32-bit is problem for the future.
        assert_eq!(size_of::<SpanBuilder>(), 32);
    }
}
