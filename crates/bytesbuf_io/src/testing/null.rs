// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::convert::Infallible;
use std::future::ready;

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
    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "API compatibility between trait and inherent fn"
    )]
    // #[gamma::skip(parameter.default_shadow, reason = "A null source ignores the requested byte count and always returns the input buffer unchanged")]
    pub async fn read_at_most_into(&mut self, _len: usize, into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        Ok((0, into))
    }

    /// Reads 0 bytes into the provided buffer, returning it as-is.
    ///
    /// # Errors
    ///
    /// This call never fails.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
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
    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "API compatibility between trait and inherent fn"
    )]
    // #[gamma::skip(fn_value.ok_default, reason = "Both the original and the mutation return Ok(BytesBuf::default())")]
    pub async fn read_any(&mut self) -> Result<BytesBuf, Infallible> {
        Ok(BytesBuf::default())
    }

    /// "Writes" the provided data, discarding it.
    ///
    /// # Errors
    ///
    /// This call never fails.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_async,
        clippy::unused_async_trait_impl,
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

    // #[gamma::skip(parameter.default_shadow, reason = "The forwarded count is ignored by the inherent null-source read")]
    async fn read_at_most_into(&mut self, len: usize, into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        self.read_at_most_into(len, into).await
    }

    async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), Infallible> {
        self.read_more_into(into).await
    }

    // #[gamma::skip(fn_value.ok_default, reason = "The inherent read returns a default BytesBuf without side effects")]
    async fn read_any(&mut self) -> Result<BytesBuf, Infallible> {
        self.read_any().await
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl Write for Null {
    type Error = Infallible;

    fn write(&mut self, _sequence: BytesView) -> impl Future<Output = Result<(), Infallible>> {
        ready(Ok(()))
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl HasMemory for Null {
    fn memory(&self) -> impl MemoryShared {
        self.memory()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
impl Memory for Null {
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
    /// Pass a [`bytesbuf::mem::GlobalPool`] directly or wrap a custom provider in [`OpaqueMemory`].
    #[must_use]
    pub fn memory(mut self, memory: impl Into<OpaqueMemory>) -> Self {
        self.memory = memory.into();
        self
    }

    /// Builds the `NullStream` with the provided configuration.
    #[must_use]
    pub fn build(self) -> Null {
        Null { memory: self.memory }
    }
}
