// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::convert::Infallible;
use std::future;

use bytesbuf::mem::testing::TransparentMemory;
use bytesbuf::mem::{HasMemory, Memory, MemoryShared, OpaqueMemory};
use bytesbuf::{BytesBuf, BytesView};

use crate::{Read, Write};

/// A [`Read`] and [`Write`] that never completes any reads or writes.
///
/// Intended for simple tests and examples that need a never-completing stream.
#[derive(Debug)]
pub struct Pending {
    memory: OpaqueMemory,
}

impl Pending {
    /// Starts building a new `PendingStream`.
    #[must_use]
    pub fn builder() -> PendingBuilder {
        PendingBuilder {
            memory: OpaqueMemory::new(TransparentMemory::new()),
        }
    }

    /// Creates a new `PendingStream` with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Starts a read operation that never completes.
    ///
    /// # Errors
    ///
    /// This call never fails (because it never completes).
    #[cfg_attr(test, mutants::skip)] // This does nothing, pointless to mutate.
    #[expect(clippy::needless_pass_by_ref_mut, reason = "API compatibility between trait and inherent fn")]
    pub async fn read_at_most_into(&mut self, _len: usize, _into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        future::pending::<()>().await;
        unreachable!();
    }

    /// Starts a read operation that never completes.
    ///
    /// # Errors
    ///
    /// This call never fails (because it never completes).
    #[cfg_attr(test, mutants::skip)] // This does nothing, pointless to mutate.
    #[expect(clippy::needless_pass_by_ref_mut, reason = "API compatibility between trait and inherent fn")]
    pub async fn read_more_into(&mut self, _into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        future::pending::<()>().await;
        unreachable!();
    }

    /// Starts a read operation that never completes.
    ///
    /// # Errors
    ///
    /// This call never fails (because it never completes).
    #[cfg_attr(test, mutants::skip)] // This does nothing, pointless to mutate.
    #[expect(clippy::needless_pass_by_ref_mut, reason = "API compatibility between trait and inherent fn")]
    pub async fn read_any(&mut self) -> Result<BytesBuf, Infallible> {
        future::pending::<()>().await;
        unreachable!();
    }

    /// Starts a write operation that never completes.
    ///
    /// # Errors
    ///
    /// This call never fails (because it never completes).
    #[cfg_attr(test, mutants::skip)] // This does nothing, pointless to mutate.
    #[expect(clippy::needless_pass_by_ref_mut, reason = "API compatibility between trait and inherent fn")]
    pub async fn write(&mut self, _sequence: BytesView) -> Result<(), Infallible> {
        future::pending::<()>().await;
        unreachable!();
    }

    /// Returns the memory provider that was configured in the builder.
    #[must_use]
    pub fn memory(&self) -> impl MemoryShared {
        self.memory.clone()
    }

    /// Reserves at least `min_bytes` bytes of memory capacity.
    ///
    /// Returns an empty [`BytesBuf`] that can be used to fill the reserved memory with data.
    ///
    /// The memory provider may provide more memory than requested.
    ///
    /// The memory reservation request will always be fulfilled, obtaining more memory from the
    /// operating system if necessary.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`BytesBuf`]
    /// with zero or more bytes of capacity.
    ///
    /// # Panics
    ///
    /// May panic if the operating system runs out of memory.
    #[must_use]
    pub fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.memory.reserve(min_bytes)
    }
}

impl Default for Pending {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl Memory for Pending {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reserve(min_bytes)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl HasMemory for Pending {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn memory(&self) -> impl MemoryShared {
        self.memory()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl Read for Pending {
    type Error = Infallible;

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    async fn read_at_most_into(&mut self, len: usize, into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        self.read_at_most_into(len, into).await
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        self.read_more_into(into).await
    }

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    async fn read_any(&mut self) -> Result<BytesBuf, Infallible> {
        self.read_any().await
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl Write for Pending {
    type Error = Infallible;

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    async fn write(&mut self, data: BytesView) -> Result<(), Infallible> {
        self.write(data).await
    }
}

/// Creates an instance of [`Pending`].
///
/// Access through [`Pending::builder()`][Pending::builder].
#[derive(Debug)]
pub struct PendingBuilder {
    memory: OpaqueMemory,
}

impl PendingBuilder {
    /// The memory provider to use in memory-related stream operations.
    ///
    /// The pending stream never reserves memory, so the only purpose of this is to allow the user
    /// of the null stream to call `memory()` and `reserve()` via the `HasMemory` and `Memory`
    /// traits that every stream implements.
    ///
    /// Optional. Defaults to using the Rust global allocator.
    #[must_use]
    pub fn memory(mut self, memory: OpaqueMemory) -> Self {
        self.memory = memory;
        self
    }

    /// Builds the `Pending` with the provided configuration.
    #[must_use]
    pub fn build(self) -> Pending {
        Pending { memory: self.memory }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Waker};

    use bytesbuf::mem::CallbackMemory;

    use super::*;

    #[test]
    fn smoke_test() {
        let stream = Pending::new();

        let reserved1 = stream.reserve(123);
        assert!(reserved1.capacity() >= 123);

        let memory = stream.memory();
        let reserved2 = memory.reserve(123);
        assert!(reserved2.capacity() >= 123);
    }

    #[test]
    fn default_returns_working_instance() {
        let stream = Pending::default();

        // Verify it can reserve memory
        let buffer = stream.reserve(100);
        assert!(buffer.remaining_capacity() >= 100);

        // Verify memory() works
        let stream_memory = stream.memory();
        let buffer2 = stream_memory.reserve(50);
        assert!(buffer2.remaining_capacity() >= 50);
    }

    #[test]
    fn memory_returns_configured_provider() {
        let callback_called = Arc::new(AtomicBool::new(false));

        let custom_memory = OpaqueMemory::new(CallbackMemory::new({
            let callback_called = Arc::clone(&callback_called);
            move |min_bytes| {
                callback_called.store(true, Ordering::SeqCst);
                TransparentMemory::new().reserve(min_bytes)
            }
        }));

        let pending_stream = Pending::builder().memory(custom_memory).build();

        // Get memory from stream and use it
        let stream_memory = pending_stream.memory();
        let _buf = stream_memory.reserve(10);

        assert!(
            callback_called.load(Ordering::SeqCst),
            "Custom memory callback should have been called"
        );
    }

    #[test]
    fn read_at_most_into_returns_pending_on_first_poll() {
        let mut stream = Pending::new();
        let buffer = BytesBuf::new();

        let mut future = pin!(stream.read_at_most_into(100, buffer));
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);

        let result = future.as_mut().poll(&mut cx);
        assert!(
            matches!(result, Poll::Pending),
            "read_at_most_into should return Pending on first poll"
        );
    }

    #[test]
    fn read_more_into_returns_pending_on_first_poll() {
        let mut stream = Pending::new();
        let buffer = BytesBuf::new();

        let mut future = pin!(stream.read_more_into(buffer));
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);

        let result = future.as_mut().poll(&mut cx);
        assert!(
            matches!(result, Poll::Pending),
            "read_more_into should return Pending on first poll"
        );
    }

    #[test]
    fn read_any_returns_pending_on_first_poll() {
        let mut stream = Pending::new();

        let mut future = pin!(stream.read_any());
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);

        let result = future.as_mut().poll(&mut cx);
        assert!(matches!(result, Poll::Pending), "read_any should return Pending on first poll");
    }

    #[test]
    fn write_returns_pending_on_first_poll() {
        let mut stream = Pending::new();
        let data = BytesView::default();

        let mut future = pin!(stream.write(data));
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);

        let result = future.as_mut().poll(&mut cx);
        assert!(matches!(result, Poll::Pending), "write should return Pending on first poll");
    }
}
