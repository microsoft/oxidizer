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

impl Write for Null {
    type Error = Infallible;

    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    async fn write(&mut self, _sequence: BytesView) -> Result<(), Infallible> {
        Ok(())
    }
}

impl HasMemory for Null {
    #[cfg_attr(test, mutants::skip)] // Trivial forwarder.
    fn memory(&self) -> impl MemoryShared {
        self.memory()
    }
}

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
mod tests {
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
}
