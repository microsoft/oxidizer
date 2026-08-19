// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::alloc::Layout;
use core::ptr::NonNull;

use crate::geometry::{SlotGeometry, TypedGeometry};
use crate::pool::PoolCore;

/// Header sitting at the base of every chunk allocation, followed (after
/// alignment padding) by the chunk's slot payload.
#[repr(C)]
pub(crate) struct ChunkHeader {
    /// Back-pointer to the owning pool, used to recover `PoolInner` from a slot
    /// pointer on the free path.
    pub(crate) pool: NonNull<PoolCore>,
    /// This chunk's first global slot index (`chunk_index * chunk_size`).
    pub(crate) base_index: u32,
    /// This chunk's position in the directory.
    pub(crate) chunk_index: u32,
}

/// Computes the [`Layout`] of a chunk holding `n` slots, or `None` on overflow.
pub(crate) fn chunk_layout<T>(n: usize) -> Option<Layout> {
    TypedGeometry::<T>::new().chunk_layout(n)
}
