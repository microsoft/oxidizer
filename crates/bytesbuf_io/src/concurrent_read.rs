// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use bytesbuf::mem::{HasMemory, Memory};

use crate::Read;

/// Allows a [`Read`] source to support multiple concurrent read operations.
///
/// The [`Read`] trait takes `&mut self`, limiting callers to one read at a time.
/// Some I/O endpoints can support many reads in flight simultaneously for higher throughput.
///
/// Implementors return a new independent [`Read`] handle from each call to [`concurrently()`],
/// enabling the caller to drive any number of reads in parallel.
///
/// [`concurrently()`]: ConcurrentRead::concurrently
pub trait ConcurrentRead: HasMemory + Memory + Debug {
    /// The type of [`Read`] handle returned by [`concurrently()`](ConcurrentRead::concurrently).
    type Handle: Read + Send + 'static;

    /// Returns a new independent [`Read`] handle that can execute one concurrent read.
    ///
    /// Each handle operates independently - multiple handles may have reads in flight at the
    /// same time without interfering with each other.
    fn concurrently(&self) -> Self::Handle;
}
