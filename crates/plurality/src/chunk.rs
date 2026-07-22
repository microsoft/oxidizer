// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "pointer-recovery and slot-lifecycle paths group tightly-coupled unsafe operations under a single documented safety invariant; one block per operation would duplicate that invariant and obscure it"
)]

use core::alloc::Layout;
use core::ptr::NonNull;

use crate::pool::PoolCore;
use crate::slot::SlotCell;

/// Header sitting at the base of every chunk allocation, followed (after
/// alignment padding) by the chunk's `[SlotCell<T>; N]` payload.
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

/// Byte offset of the slot payload within a chunk allocation. Independent of
/// the chunk size, so recovery (and slot addressing) is pure arithmetic.
const fn slots_offset<T>() -> usize {
    let header = size_of::<ChunkHeader>();
    let align = align_of::<SlotCell<T>>();
    header.next_multiple_of(align)
}

/// Alignment of a whole chunk allocation.
#[cfg_attr(test, mutants::skip)] // `>` vs `>=` is equivalent here: when `ha == sa` both arms return the same value.
const fn chunk_align<T>() -> usize {
    let ha = align_of::<ChunkHeader>();
    let sa = align_of::<SlotCell<T>>();
    if ha > sa { ha } else { sa }
}

/// Computes the [`Layout`] of a chunk holding `n` slots, or `None` on overflow.
pub(crate) fn chunk_layout<T>(n: usize) -> Option<Layout> {
    let align = chunk_align::<T>();
    debug_assert!(
        align >= align_of::<ChunkHeader>() && align >= align_of::<SlotCell<T>>(),
        "chunk alignment must cover the header and the slots",
    );
    let slots = size_of::<SlotCell<T>>().checked_mul(n)?;
    let size = slots_offset::<T>().checked_add(slots)?;
    Layout::from_size_align(size, align).ok().map(|l| l.pad_to_align())
}

/// Returns the slot at `offset` within the chunk whose header is `chunk`.
///
/// # Safety
/// `chunk` must point at a live chunk and `offset` must be `< N`.
#[inline]
pub(crate) unsafe fn slot_at<T>(chunk: NonNull<ChunkHeader>, offset: usize) -> NonNull<SlotCell<T>> {
    // SAFETY: the payload begins `slots_offset` bytes into the chunk and holds
    // at least `offset + 1` slots by the caller's contract.
    unsafe {
        let first = chunk.as_ptr().cast::<u8>().add(slots_offset::<T>()).cast::<SlotCell<T>>();
        NonNull::new_unchecked(first.add(offset))
    }
}

/// Recovers the owning chunk header from a slot pointer and its (already read)
/// in-chunk index, by arithmetic.
///
/// # Safety
/// `slot` must point at a live slot belonging to a chunk laid out by this
/// crate, and `index` must be that slot's stored in-chunk index.
#[inline]
#[expect(
    clippy::cast_ptr_alignment,
    reason = "the recovered chunk header sits at a `ChunkHeader`-aligned offset by construction of the chunk layout"
)]
pub(crate) unsafe fn header_of<T>(slot: NonNull<SlotCell<T>>, index: u32) -> NonNull<ChunkHeader> {
    // SAFETY: `index` is the slot's in-chunk position, so stepping back that
    // many slots lands on the first slot, and stepping back `slots_offset`
    // bytes lands on the chunk header.
    unsafe {
        let first = slot.as_ptr().sub(index as usize);
        let header = first.cast::<u8>().sub(slots_offset::<T>()).cast::<ChunkHeader>();
        NonNull::new_unchecked(header)
    }
}
