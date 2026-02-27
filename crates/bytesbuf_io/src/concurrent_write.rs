// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use bytesbuf::mem::{HasMemory, Memory};

use crate::Write;

/// Allows a [`Write`] sink to support multiple concurrent write operations.
///
/// The [`Write`] trait takes `&mut self`, limiting callers to one write at a time.
/// Some I/O endpoints can support many writes in flight simultaneously for higher throughput.
///
/// Implementors return a new independent [`Write`] handle from each call to [`concurrently()`],
/// enabling the caller to drive any number of writes in parallel.
///
/// [`concurrently()`]: ConcurrentWrite::concurrently
pub trait ConcurrentWrite: HasMemory + Memory + Debug {
    /// The type of [`Write`] handle returned by [`concurrently()`](ConcurrentWrite::concurrently).
    type Handle: Write + Send + 'static;

    /// Returns a new independent [`Write`] handle that can execute one concurrent write.
    ///
    /// Each handle operates independently - multiple handles may have writes in flight at the
    /// same time without interfering with each other.
    fn concurrently(&self) -> Self::Handle;
}
