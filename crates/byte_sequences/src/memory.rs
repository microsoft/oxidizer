// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::SequenceBuilder;

/// Provides memory capacity for byte sequences.
///
/// Call [`reserve()`][Self::reserve] to reserve memory capacity and obtain a [`SequenceBuilder`]
/// that can be used to fill the reserved memory with data.
#[doc = include_str!("../doc/snippets/choosing_memory_provider.md")]
///
/// # Resource management
///
/// The reserved memory is released when the last [`SequenceBuilder`] or
/// [`Sequence`][crate::Sequence] referencing it is dropped.
pub trait Memory: Debug {
    /// Reserves at least `min_bytes` bytes of memory capacity.
    ///
    /// Returns an empty [`SequenceBuilder`] that can be used to fill the reserved memory with data.
    ///
    /// The memory provider may provide more memory than requested.
    ///
    /// The memory reservation request will always be fulfilled, obtaining more memory from the
    /// operating system if necessary.
    ///
    /// # Zero-sized reservations
    ///
    /// Reserving zero bytes of memory is a valid operation and will return a [`SequenceBuilder`]
    /// with zero or more bytes of capacity.
    ///
    /// # Panics
    ///
    /// May panic if the operating system runs out of memory.
    #[must_use]
    fn reserve(&self, min_bytes: usize) -> SequenceBuilder;
}
