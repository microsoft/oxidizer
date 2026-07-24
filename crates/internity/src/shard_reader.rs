// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ShardReader`]: one shard's strings in a frozen, flat layout.

use alloc::boxed::Box;

/// A single shard's interned strings, frozen into a contiguous layout: all bytes
/// concatenated in `bytes`, with CSR-style `offsets` — `offsets[i]` the start and
/// `offsets[i+1]` the end of the `i`-th string (leading `0` sentinel,
/// `len() + 1` entries). Immutable — no locks, no atomics, no per-index branch.
pub(crate) struct ShardReader {
    offsets: Box<[u32]>,
    bytes: Box<[u8]>,
}

impl ShardReader {
    pub(crate) fn new(offsets: Box<[u32]>, bytes: Box<[u8]>) -> Self {
        Self { offsets, bytes }
    }

    /// Number of strings in this shard.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    /// Resolves a 0-based local index to its string, or `None` if out of range.
    #[inline]
    pub(crate) fn get(&self, local: usize) -> Option<&str> {
        crate::storage::resolve(&self.offsets, &self.bytes, local)
    }
}
