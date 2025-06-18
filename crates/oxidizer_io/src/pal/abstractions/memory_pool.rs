// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::num::NonZeroUsize;

use crate::mem::SequenceBuilder;

/// Provides I/O blocks that back byte sequences used by the I/O subsystem for input/output. The
/// pool has a dynamic size and will expand to meet demand.
///
/// It is expected that the memory pool optimizes memory allocations so that the memory is
/// optimized for usage with the operating system's I/O APIs (e.g. page-aligned). The pool is
/// only meant to be used for I/O blocks that back I/O byte sequences.
///
/// The memory pool is generic over the memory model and will customize its behavior to match the
/// memory model in use.
///
/// # Ownership
///
/// The memory pool is owned by the I/O resource pool, which in turn is a shared resource owned by
/// both I/O clients and the I/O driver itself.
///
/// The type uses interior mutability to present an easy to use shared API surface.
pub trait MemoryPool: Debug {
    /// Rents at least `count_bytes` of memory from the pool, in the form of one or more I/O blocks
    /// assembled into a [`SequenceBuilder`].
    ///
    /// The preferred block size indicated by the caller is a hint to the pool, allowing it to make
    /// optimal memory layout decisions. The pool may choose to ignore the hint and use a different
    /// block size (either smaller or larger). Callers are required to work with blocks of any size,
    /// including as small as blocks of 1 byte each.
    ///
    /// The pool will use as many blocks as necessary to provide the requested capacity. If
    /// insufficient memory is available in the pool to satisfy the request, the pool will
    /// allocate more memory.
    fn rent(&self, count_bytes: usize, preferred_block_size: NonZeroUsize) -> SequenceBuilder;
}