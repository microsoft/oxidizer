// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::convert::Infallible;

use bytesbuf::mem::testing::TransparentMemory;
use bytesbuf::mem::{HasMemory, Memory, MemoryShared, OpaqueMemory};
use bytesbuf::{BytesBuf, BytesView};

use crate::{Read, Write};

/// A [`Read`] and [`Write`] that does nothing.
///
/// Any data written to it is discarded and it never returns any data when read from.
/// Intended for simple tests and examples only.
#[derive(Debug)]
pub struct Null {
    memory: OpaqueMemory,
}

impl Null {
    /// Starts building a new `NullStream`.
    #[must_use]
    pub fn builder() -> NullBuilder {
        NullBuilder {
            memory: OpaqueMemory::new(TransparentMemory::new()),
        }
    }

    /// Creates a new `NullStream` with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Reads 0 bytes into the provided buffer, returning it as-is.
    ///
    /// # Errors
    ///
    /// This call never fails.
    #[cfg_attr(test, mutants::skip)] // This does nothing, pointless to mutate.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_async,
        reason = "API compatibility between trait and inherent fn"
    )]
    pub async fn read_at_most_into(&mut self, _len: usize, into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        Ok((0, into))
    }

    /// Reads 0 bytes into the provided buffer, returning it as-is.
    ///
    /// # Errors
    ///
    /// This call never fails.
    #[cfg_attr(test, mutants::skip)] // This does nothing, pointless to mutate.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_async,
        reason = "API compatibility between trait and inherent fn"
    )]
    pub async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        Ok((0, into))
    }

    /// Reads 0 bytes.
    ///
    /// # Errors
    ///
    /// This call never fails.
    #[cfg_attr(test, mutants::skip)] // This does nothing, pointless to mutate.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_async,
        reason = "API compatibility between trait and inherent fn"
    )]
    pub async fn read_any(&mut self) -> Result<BytesBuf, Infallible> {
        Ok(BytesBuf::default())
    }

    /// "Writes" the provided data, discarding it.
    ///
    /// # Errors
    ///
    /// This call never fails.
    #[cfg_attr(test, mutants::skip)] // This does nothing, pointless to mutate.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_async,
        reason = "API compatibility between trait and inherent fn"
    )]
    pub async fn write(&mut self, _sequence: BytesView) -> Result<(), Infallible> {
        Ok(())
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

impl Default for Null {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl Read for Null {
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
impl Write for Null {
    type Error = Infallible;

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    async fn write(&mut self, _sequence: BytesView) -> Result<(), Infallible> {
        Ok(())
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl HasMemory for Null {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn memory(&self) -> impl MemoryShared {
        self.memory()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl Memory for Null {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reserve(min_bytes)
    }
}

/// Creates an instance of [`Null`].
///
/// Access through [`Null::builder()`][Null::builder].
#[derive(Debug)]
pub struct NullBuilder {
    memory: OpaqueMemory,
}

impl NullBuilder {
    /// The memory provider to use in memory-related stream operations.
    ///
    /// The null stream never reserves memory, so the only purpose of this is to allow the user
    /// of the null stream to call `memory()` and `reserve()` via the `HasMemory` and `Memory`
    /// traits that every stream implements.
    ///
    /// Optional. Defaults to using the Rust global allocator.
    #[must_use]
    pub fn memory(mut self, memory: OpaqueMemory) -> Self {
        self.memory = memory;
        self
    }

    /// Builds the `NullStream` with the provided configuration.
    #[must_use]
    pub fn build(self) -> Null {
        Null { memory: self.memory }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use bytesbuf::mem::CallbackMemory;
    use testing_aids::execute_or_terminate_process;

    use super::*;

    #[test]
    fn smoke_test() {
        execute_or_terminate_process(|| {
            futures::executor::block_on(async {
                let mut s = Null::new();

                let buffer = s.reserve(1000);
                assert!(buffer.remaining_capacity() >= 1000);

                let (bytes_read, buffer) = s.read_at_most_into(100, buffer).await.unwrap();
                assert_eq!(bytes_read, 0);
                assert_eq!(buffer.len(), 0);

                let (bytes_read, buffer) = s.read_more_into(buffer).await.unwrap();
                assert_eq!(bytes_read, 0);
                assert_eq!(buffer.len(), 0);

                let mut buffer = s.read_any().await.unwrap();
                assert_eq!(buffer.len(), 0);

                s.write(buffer.consume_all()).await.unwrap();
            });
        });
    }

    #[test]
    fn default_returns_working_instance() {
        execute_or_terminate_process(|| {
            futures::executor::block_on(async {
                let mut s = Null::default();

                // Verify it can reserve memory
                let buffer = s.reserve(100);
                assert!(buffer.remaining_capacity() >= 100);

                // Verify read operations work
                let (bytes_read, _) = s.read_at_most_into(10, BytesBuf::new()).await.unwrap();
                assert_eq!(bytes_read, 0);

                // Verify write operations work
                let empty_view = BytesView::default();
                s.write(empty_view).await.unwrap();
            });
        });
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

        let null_stream = Null::builder().memory(custom_memory).build();

        // Get memory from stream and use it
        let stream_memory = null_stream.memory();
        let _buf = stream_memory.reserve(10);

        assert!(
            callback_called.load(Ordering::SeqCst),
            "Custom memory callback should have been called"
        );
    }
}
