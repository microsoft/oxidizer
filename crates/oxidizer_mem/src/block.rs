// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::{Layout, alloc, dealloc};
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::ptr::NonNull;
use std::sync::Arc;

#[cfg(test)]
use crate::SpanBuilder;
use crate::ThreadSafe;

/// The maximum size of an I/O block providing memory for a byte sequence.
///
/// Various operating system I/O APIs are limited to `u32::MAX`, so we match it, even though we use
/// usize internally, which in principle could accommodate more memory per span. This is not a
/// big problem because in reality our memory blocks are tiny (measured in kilobytes) and we can
/// use many I/O blocks in sequence to represent larger quantities of data.
pub const MAX_BLOCK_SIZE: usize = u32::MAX as usize;

/// An I/O block is the memory allocation unit of the I/O subsystem.
///
/// A block is a bookkeeping structure around a contiguous chunk of memory. Most importantly, the
/// metadata it carries is used to identify when the block is to be returned to the pool it came
/// from once all references are dropped.
///
/// When in use, the contents of a block are always accessed via either:
///
/// 1. Any number of `Spans` over immutable data.
/// 1. At most one `SpanBuilder` over mutable memory (which may be partly filled with
///    data and partly uninitialized).
///
/// When the I/O block is first returned from a memory pool, the `SpanBuilder` is created.
/// From this, callers may detach `Span`s from the front to create immutable sub-slices of a
/// desired length. The `Span`s may later be cloned/sliced without constraints.
///
/// Note: `Block`, `Span` and `SpanBuilder` are private APIs and not exposed in the public API
/// surface. The public API only works with `Sequence` and `SequenceBuilder`.
///
/// # Ownership
///
/// I/O blocks are passed by `Arc` to avoid lifetime parameter pollution. This also implies
/// that we are not using the Rust borrow checker to enforce exclusive reference semantics. Instead,
/// we rely on the guarantees provided by the Span/SpanBuilder types to ensure no forbidden mode
/// of access takes place. This is supported by the following guarantees:
///
/// 1. The only way to write to an I/O block is to own a [`SpanBuilder`] that can be used
///    to append data to the I/O block on the fly via `bytes::BufMut` or to read into the I/O block
///    via elementary I/O operations issued to the operating system (typically via participating
///    in a [`SequenceBuilder`] vectored read that fills multiple I/O blocks simultaneously).
/// 2. Reading from an I/O block is only possible once the I/O block (or a slice of it) has been
///    filled with data and the filled region separated into a [`Span`], detaching it from
///    the [`SpanBuilder`]. At this point further mutation of the detached slice is impossible.
///
/// All of this applies to individual slices of I/O blocks. That is, when the first part of an
/// I/O block is filled with data and detached from [`SpanBuilder`] as an [`Span`], that part
/// becomes immutable but the remainder of the I/O block may still contain writable capacity.
///
/// Phrasing this in terms of (imaginary) reference ownership semantics:
///
/// * At most one [`SpanBuilder`] has a `&mut [MaybeUninit<u8>]` to the part of the I/O
///   block that has not yet been filled with data.
/// * At most one [`SpanBuilder`] has a `&[u8]` to the part of the I/O block that has already been
///   filled with data (or a sub-slice of it, if parts have been detached from the front).
/// * Any number of [`Span`]s have a `&[u8]` to the part of the I/O block that has already
///   been filled with data (or sub-slices of it, potentially different sub-slices each).
///
/// In each of these cases, the ownership semantics are mediated via [`Span`] and
/// [`SpanBuilder`] instances that perform the bookkeeping necessary to implement the
/// ownership model. The I/O block itself is ignorant of all this machinery, merely handing out an
/// [`SpanBuilder`] for its contents when rented from a memory pool.
#[derive(Debug)]
pub struct Block {
    data: ThreadSafe<NonNull<MaybeUninit<u8>>>,
    size: NonZero<usize>,
    // Note the lack of any actual ownership bookkeeping. That's right, the memory pool of today
    // is a scam - we allocate new blocks instead of reusing them! Major efficiency crime!
    // One day it will be better, though, and the API should stay similar enough, so this is fine.
}

impl Block {
    fn layout(size: NonZero<usize>) -> Layout {
        Layout::array::<MaybeUninit<u8>>(size.get())
            .expect("if array layout fails, the universe is doomed already")
    }

    /// Obtains the contents of the I/O block as a [`SpanBuilder`] that can be used to fill the
    /// block with data and to create immutable views over the filled bytes.
    ///
    /// # Safety
    ///
    /// This must only be called when there are no existing references to the I/O block contents,
    /// as a [`SpanBuilder`] requires exclusive access when taking ownership.
    #[cfg(test)] // Currently only used in tests.
    pub(crate) const unsafe fn take_ownership(self: Arc<Self>) -> SpanBuilder {
        // TODO: Simplify the safety semantics, so we assert exclusive ownership instead of
        // requiring the caller to ensure it. Requires ownership bookkeeping to be implemented.

        SpanBuilder::new(self)
    }

    pub(crate) fn new(size: NonZero<usize>) -> Arc<Self> {
        assert!(
            size.get() <= MAX_BLOCK_SIZE,
            "requested block size exceeds internal API compatibility thresholds"
        );

        let layout = Self::layout(size);

        // SAFETY: Layout must have nonzero size. We take as input a `size` that has a `NonZero`
        // type, which should eliminate any possibility of allocating zero-sized layouts.
        let data = unsafe { alloc(layout) }.cast::<MaybeUninit<u8>>();

        let data = NonNull::new(data).expect("allocation failure is a fatal error");

        // SAFETY: It is just a pointer that we own, we treat it thread-safely in all the ways.
        // Ownership semantics of the contents are enforced by the `Span` and `SpanBuilder` types.
        let data = unsafe { ThreadSafe::new(data) };

        Arc::new(Self { data, size })
    }

    /// Size of the I/O block in bytes.
    ///
    /// The I/O block only knows about its size, not about the contents. It also does not know
    /// whether the contents are initialized or not. Any contents-oriented logic is handled by
    /// the [`Span`] and [`SpanBuilder`] types.
    pub(crate) const fn size(&self) -> NonZero<usize> {
        self.size
    }

    #[expect(
        clippy::missing_const_for_fn,
        reason = "Analysis is wrong, this can't be made const based on current Rust rules"
    )]
    pub(crate) fn as_ptr(&self) -> NonNull<MaybeUninit<u8>> {
        *self.data
    }
}

impl Drop for Block {
    // Infeasible to test that dealloc happened. If we enhance this with more functionality,
    // testability might increase (e.g. to test ownership bookkeeping).
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // TODO: Assert that nobody is still using it.

        let layout = Self::layout(self.size);

        // SAFETY: We are required to pass matching inputs with regard to alloc(). We do.
        unsafe {
            dealloc(self.data.as_ptr().cast::<u8>(), layout);
        }
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::*;

    #[test]
    fn smoke_test() {
        let block = Block::new(NonZero::new(1234).unwrap());
        drop(block);
    }

    #[test]
    fn thread_safe_type() {
        assert_impl_all!(Block: Send, Sync);
    }
}