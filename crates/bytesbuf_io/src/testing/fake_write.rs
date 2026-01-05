// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::convert::Infallible;

use bytesbuf::mem::testing::TransparentMemory;
use bytesbuf::mem::{HasMemory, Memory, MemoryShared, OpaqueMemory};
use bytesbuf::{BytesBuf, BytesView};

use crate::Write;

/// A [`Write`] that collects all written data into itself.
///
/// This is for test and example purposes only and is not optimized for performance.
#[derive(Debug)]
pub struct FakeWrite {
    contents: BytesBuf,

    memory: OpaqueMemory,
}

impl FakeWrite {
    /// Starts building a new `FakeWrite`.
    #[must_use]
    pub fn builder() -> FakeWriteBuilder {
        FakeWriteBuilder {
            memory: OpaqueMemory::new(TransparentMemory::new()),
        }
    }

    /// Creates a new `FakeWrite` with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Consumes the instance and returns a [`BytesView`] with the contents that were written to it.
    #[must_use]
    pub fn into_contents(mut self) -> BytesView {
        self.contents.consume_all()
    }

    /// References the contents written into the stream so far.
    ///
    /// The contents are stored in a `BytesBuf` which may be inspected by the caller.
    #[must_use]
    pub fn contents(&self) -> &BytesBuf {
        &self.contents
    }

    /// Writes the provided byte sequence to the stream.
    ///
    /// # Errors
    ///
    /// This call never fails.
    #[expect(clippy::unused_async, reason = "API compatibility between trait and inherent fn")]
    pub async fn write(&mut self, data: BytesView) -> Result<(), Infallible> {
        self.contents.put_bytes(data);
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

impl Default for FakeWrite {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl Write for FakeWrite {
    type Error = Infallible;

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    async fn write(&mut self, data: BytesView) -> Result<(), Infallible> {
        self.write(data).await
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl Memory for FakeWrite {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reserve(min_bytes)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl HasMemory for FakeWrite {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn memory(&self) -> impl MemoryShared {
        self.memory()
    }
}

/// Creates an instance of [`FakeWrite`].
///
/// Access through [`FakeWrite::builder()`][FakeWrite::builder].
#[derive(Debug)]
pub struct FakeWriteBuilder {
    memory: OpaqueMemory,
}

impl FakeWriteBuilder {
    /// The memory provider to use in memory-related stream operations.
    ///
    /// Optional. Defaults to using the Rust global allocator.
    #[must_use]
    pub fn memory(mut self, memory: OpaqueMemory) -> Self {
        self.memory = memory;
        self
    }

    /// Builds the `FakeWrite` with the provided configuration.
    #[must_use]
    pub fn build(self) -> FakeWrite {
        FakeWrite {
            contents: BytesBuf::new(),
            memory: self.memory,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use bytesbuf::mem::CallbackMemory;
    use testing_aids::async_test;

    use super::*;
    use crate::WriteExt;

    #[test]
    fn smoke_test() {
        async_test(async || {
            let mut write_stream = FakeWrite::new();

            write_stream
                .prepare_and_write(1234, |mut buf| {
                    buf.put_byte(1);
                    buf.put_byte(2);
                    buf.put_byte(3);
                    Ok::<BytesView, Infallible>(buf.consume_all())
                })
                .await
                .unwrap();

            assert_eq!(write_stream.contents().len(), 3);

            let mut contents = write_stream.into_contents();
            assert_eq!(contents.len(), 3);

            assert_eq!(contents.get_byte(), 1);
            assert_eq!(contents.get_byte(), 2);
            assert_eq!(contents.get_byte(), 3);
            assert_eq!(contents.len(), 0);
        });
    }

    #[test]
    fn default_returns_working_instance() {
        async_test(async || {
            let mut write_stream = FakeWrite::default();

            write_stream
                .prepare_and_write(10, |mut buf| {
                    buf.put_byte(42);
                    Ok::<BytesView, Infallible>(buf.consume_all())
                })
                .await
                .unwrap();

            assert_eq!(write_stream.contents().len(), 1);

            let mut contents = write_stream.into_contents();
            assert_eq!(contents.get_byte(), 42);
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

        let write_stream = FakeWrite::builder().memory(custom_memory).build();

        // Get memory from stream and use it
        let stream_memory = write_stream.memory();
        let _buf = stream_memory.reserve(10);

        assert!(callback_called.load(Ordering::SeqCst), "Custom memory callback should have been called");
    }
}
