// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::slice;
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::ptr::NonNull;

use crate::{BlockRef, BlockSize, Span};

/// Owns a mutable span of memory capacity from a memory block, which can be filled with data,
/// enabling you to detach spans of immutable bytes from the front to create views over the data.
///
/// Use `unfilled_slice_mut()` and `advance()` to fill available memory capacity with data,
/// after which you may detach spans of immutable data from the front via [`consume()`].
///
/// Filled bytes may be inspected via [`peek()`] to enable a content-based determination
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
/// or mutable memory. The mutable memory is always at the end of the block - blocks are filled
/// from the start.
///
/// These parts are always accessed via either:
///
/// 1. Any number of `Spans`, each owning an arbitrary range of immutable data from the block.
/// 1. At most one `SpanBuilder`, owning:
///    * At most one range of immutable data from the block.
///    * At most one range of mutable memory from the block.
///
/// When a memory block is first put into use, one `SpanBuilder` is created and has exclusive
/// ownership of the block. From this builder, callers may detach `Span`s from the front to create
/// sub-slices over immutable data of a desired length. The `Span`s may later be cloned/sliced
/// without constraints. The `SpanBuilder` retains exclusive ownership of the remaining part
/// of the memory block (the part that has not been detached as a `Span`), though it may surrender
/// some of that exclusivity early via `peek()`, which creates a `Span` while still keeping the
/// data inside the `SpanBuilder`.
///
/// Note: `Span` and `SpanBuilder` are private APIs and not exposed in the public API surface.
/// The public API only works with `BytesView` and `BytesBuf`. The only purpose of these types
/// is to implement the internal memory ownership mechanics of `BytesBuf` and `BytesView`.
///
/// Memory blocks are reference counted to avoid lifetime parameter pollution and provide
/// flexibility in usage. This also implies that we are not using the Rust borrow checker to
/// enforce exclusive reference semantics. Instead, we rely on the guarantees provided by the
/// [`Span`]/[`SpanBuilder`] types to ensure no forbidden mode of access takes place. This is supported
/// by the following guarantees:
///
/// 1. The only way to write to a memory block is to own a [`SpanBuilder`] that can be used
///    to either copy data into the block or to transfer data into it via I/O operations issued
///    to the operating system (typically via participating in a [`BytesBuf`][crate::BytesBuf]
///    vectored read that fills multiple memory blocks simultaneously).
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
/// * At most one [`SpanBuilder`] has a `&[u8]` to the part of the memory block that has already
///   been filled with data (or a sub-slice of it, if some bytes have been detached from the front).
/// * Any number of [`Span`]s have a `&[u8]` to the part of the memory block that has already
///   been filled with data (or sub-slices of it, potentially different sub-slices each).
///
/// In each of these cases, the ownership semantics are mediated via [`Span`] and
/// [`SpanBuilder`] instances that perform the bookkeeping necessary to implement the
/// ownership model. The memory block object itself is ignorant of all this machinery, merely being
/// a reference counting structure around the pointer and length that designates the capacity, with
/// one `BlockRef` indicating one reference. Once all `BlockRef` instances are dropped, the memory
/// provider it came from will reclaim the memory block for reuse or release.
#[derive(Debug)]
pub(crate) struct SpanBuilder {
    // For the purposes of the `Span` and `SpanBuilder` types, this merely controls the lifecycle
    // of the memory block - dropping the last reference will permit the memory block to be
    // reclaimed by the memory provider it originates from.
    block_ref: BlockRef,

    // Pointer to the start of the span builder's capacity. This region includes both
    // the filled bytes and the available bytes.
    //
    // Any bytes that have been consumed from the span builder (in the form of `Span` instances)
    // are no longer accessible through this pointer - they are not considered part of the
    // builder's capacity and instead become part of the detached span's capacity.
    start: NonNull<MaybeUninit<u8>>,

    // Number of bytes after `start` that have been filled with data.
    // Any bytes after this range must be treated as uninitialized.
    filled_bytes: BlockSize,

    // Number of bytes after `start + filled_bytes` that are available to be filled with data.
    // This range of bytes must be treated as uninitialized.
    available_bytes: BlockSize,
}

impl SpanBuilder {
    /// Creates a span builder and gives it exclusive ownership of a memory block.
    ///
    /// The `BlockRef` acts as an `Arc`-style reference counted handle to the memory block.
    /// It may be cloned at any time to share ownership of the memory block with `Span`
    /// instances created by the `SpanBuilder`.
    ///
    /// When the last instance from the family of `BlockRef` clones is dropped, the
    /// memory capacity associated with the memory block will be reclaimed by the
    /// memory provider it originates from.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the `SpanBuilder` being created has exclusive ownership
    /// of the provided memory blocks. This means that no `BlockRef` clones referencing the
    /// same block are permitted to exist at this point.
    pub(crate) const unsafe fn new(start: NonNull<MaybeUninit<u8>>, len: NonZero<BlockSize>, block_ref: BlockRef) -> Self {
        Self {
            block_ref,
            start,
            filled_bytes: 0,
            available_bytes: len.get(),
        }
    }

    /// Bytes of data contained in the span builder.
    ///
    /// These bytes may be consumed, detaching `Span` instances from the front via `consume()`.
    pub(crate) const fn len(&self) -> BlockSize {
        self.filled_bytes
    }

    /// Returns `true` if there are no filled bytes in the span builder.
    ///
    /// There may still be available capacity to fill with data.
    pub(crate) const fn is_empty(&self) -> bool {
        self.filled_bytes == 0
    }

    /// How many more bytes of data fit into the span builder.
    #[cfg_attr(test, mutants::skip)] // Lying about capacity is a great way to infinite loop.
    pub(crate) fn remaining_capacity(&self) -> usize {
        self.available_bytes as usize
    }

    /// Consumes bytes of data from the front of the span builder.
    ///
    /// The span builder's memory capacity shrinks by bytes consumed, with the returned bytes
    /// no longer having any association with the span builder.
    ///
    /// # Panics
    ///
    /// Panics if the requested number of bytes is greater than `len()`.
    pub(crate) fn consume(&mut self, len: NonZero<BlockSize>) -> Span {
        assert!(len.get() <= self.len());

        // SAFETY: We must guarantee that the range to return has been initialized.
        // Yes, it has - this is guarded by the `length` check above.
        let span = unsafe { Span::new(self.start.cast(), len.get(), self.block_ref.clone()) };

        // This cannot overflow - guarded by assertion above.
        self.filled_bytes = self.filled_bytes.wrapping_sub(len.get());

        // SAFETY: Above, we only seek over filled bytes, so we must still be in-bounds.
        self.start = unsafe { self.start.add(len.get() as usize) };

        span
    }

    /// Creates a `Span` over the data in the span builder without consuming it from the builder.
    pub(crate) fn peek(&self) -> Span {
        // SAFETY: The data in the span builder up to `filled_bytes` is initialized.
        unsafe { Span::new(self.start.cast(), self.filled_bytes, self.block_ref.clone()) }
    }

    /// References the memory block that provides the span builder's memory capacity.
    pub(crate) const fn block(&self) -> &BlockRef {
        &self.block_ref
    }

    /// References the available (unfilled) part of the span builder's memory capacity.
    #[cfg_attr(test, mutants::skip)] // Risk of UB if this returns wrong slices.
    pub(crate) fn unfilled_slice_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        // SAFETY: We are seeking past initialized memory, so at most we are at the end of our
        // memory block (which is still valid) but cannot exceed it in any way.
        let available_start = unsafe { self.start.add(self.filled_bytes as usize) };

        // SAFETY: We are responsible for the pointer pointing to a valid storage of the given type
        // (guaranteed by memory block) and for the slice having exclusive access to the memory for
        // the duration of its lifetime (guaranteed by `&mut self` which inherits exclusive access
        // from the SpanBuilder itself, which received such a guarantee in `new()`). While part
        // of the span builder's memory may already be shared via `Span` instances created as a
        // result of `peek()`, we seek over those ranges and only return the unfilled part here.
        unsafe { slice::from_raw_parts_mut(available_start.as_ptr(), self.available_bytes as usize) }
    }

    /// Signals that `len` bytes of data has been written into the span builder.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that at least `len` bytes of data has been written
    /// at the start of the unfilled slice.
    ///
    /// The caller must guarantee that `len` is less than or equal to `remaining_capacity()`.
    pub(crate) unsafe fn advance(&mut self, len: usize) {
        #[expect(clippy::cast_possible_truncation, reason = "guaranteed by safety requirements")]
        let count = len as BlockSize;

        // Cannot overflow - guaranteed by safety requirements.
        self.available_bytes = self.available_bytes.wrapping_sub(count);

        // Cannot overflow - guaranteed by safety requirements.
        self.filled_bytes = self.filled_bytes.wrapping_add(count);
    }

    /// Appends a slice of bytes to the span builder.
    ///
    /// Convenience function for testing purposes, to allow a span builder to be easily
    /// filled with test data. In real usage, all filling of data will occur through the
    /// methods on `BytesBuf`.
    #[cfg(test)]
    pub(crate) fn put_slice(&mut self, src: &[u8]) {
        use std::ptr;

        let len = src.len();

        assert!(self.remaining_capacity() >= len);

        let dest_slice = self.unfilled_slice_mut();

        // SAFETY: Both are byte slices, so no alignment concerns.
        // We verified length is in bounds above.
        unsafe {
            ptr::copy_nonoverlapping(src.as_ptr(), dest_slice.as_mut_ptr().cast(), len);
        }

        // SAFETY: We indeed filled this many bytes and verified there was enough capacity.
        unsafe { self.advance(len) };
    }
}

// SAFETY: The presence of pointers disables Send but we re-enable it here because all our internal
// state is thread-mobile.
unsafe impl Send for SpanBuilder {}
// SAFETY: The presence of pointers disables Sync but we re-enable it here because all our internal
// state is thread-safe (though only for reads - we still require outer mutability, which disables
// multithreaded mutation).
unsafe impl Sync for SpanBuilder {}

#[cfg(test)]
mod tests {
    use new_zealand::nz;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::std_alloc_block;

    // The type is thread-mobile (Send) and can be shared (for reads) between threads (Sync).
    assert_impl_all!(SpanBuilder: Send, Sync);

    #[test]
    fn smoke_test() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        assert_eq!(builder.len(), 0);
        assert!(builder.is_empty());
        assert_eq!(builder.remaining_capacity(), 10);

        builder.put_slice(&1234_u64.to_ne_bytes());

        assert_eq!(builder.len(), 8);
        assert!(!builder.is_empty());
        assert_eq!(builder.remaining_capacity(), 2);

        _ = builder.consume(nz!(8));

        assert_eq!(builder.len(), 0);
        assert!(builder.is_empty());
        assert_eq!(builder.remaining_capacity(), 2);

        builder.put_slice(&1234_u16.to_ne_bytes());

        assert_eq!(builder.len(), 2);
        assert!(!builder.is_empty());
        assert_eq!(builder.remaining_capacity(), 0);

        _ = builder.consume(nz!(2));

        assert_eq!(builder.len(), 0);
        assert!(builder.is_empty());
        assert_eq!(builder.remaining_capacity(), 0);
    }

    #[test]
    fn peek() {
        let mut builder = std_alloc_block::allocate(nz!(10)).into_span_builder();

        builder.put_slice(&1234_u32.to_ne_bytes());
        builder.put_slice(&5678_u32.to_ne_bytes());
        builder.put_slice(&90_u16.to_ne_bytes());

        let mut peeked = builder.peek();

        assert_eq!(peeked.len(), 10);
        assert_eq!(peeked.as_ref().len(), 10);

        assert_eq!(u32::from_ne_bytes(peeked.get_array()), 1234);
        assert_eq!(u32::from_ne_bytes(peeked.get_array()), 5678);
        assert_eq!(u16::from_ne_bytes(peeked.get_array()), 90);

        assert_eq!(peeked.len(), 0);
        assert_eq!(peeked.as_ref().len(), 0);

        _ = builder.consume(nz!(10));
    }

    #[test]
    fn size_change_detector() {
        // The point of this is not to say that we expect it to have a specific size but to allow
        // us to easily detect when the size changes and (if we choose to) bless the change.
        // We assume 64-bit pointers - any support for 32-bit is problem for the future.
        assert_eq!(size_of::<SpanBuilder>(), 32);
    }
}
