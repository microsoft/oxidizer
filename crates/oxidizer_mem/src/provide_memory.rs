// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::SequenceBuilder;

/// Allows mutable memory to be requested from memory providers, supporting efficient memory pooling
/// and custom memory allocation strategies.
///
/// This trait provides a general mechanism for requesting memory from various memory providers.
/// While commonly used in I/O scenarios, it can be applied to any situation requiring optimized
/// memory allocation and reuse. Memory providers can implement custom allocation strategies
/// tailored to specific use cases.
///
/// The crate provides [`DefaultMemoryPool`][crate::DefaultMemoryPool] as a general-purpose implementation that allocates
/// memory from the Rust allocator with configurable block sizes.
///
/// # Usage
///
/// This trait can be used in various scenarios:
///
/// - Memory pooling for performance-critical applications
/// - Custom memory allocation with specific layout requirements
/// - I/O operations where memory layout affects performance
/// - Situations where memory needs to be prepared before being consumed by an operation
///
/// # Resource management
///
/// The reserved memory is released when the last [`SequenceBuilder`] and [`Sequence`][crate::Sequence] reference
/// to it is dropped. Memory may be reserved and released in multiple pieces with independent
/// lifetimes, so dropping references to only part of the reserved capacity may still be beneficial
/// in allowing some of the resources to be released.
pub trait ProvideMemory: Debug {
    /// Reserves at least `min_bytes` bytes of mutable memory, returning a [`SequenceBuilder`]
    /// whose capacity is backed by the reserved memory.
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
    fn reserve(&self, min_bytes: usize) -> SequenceBuilder;
}