// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared raw-allocation helpers used by `LocalChunk::allocate` and
//! `SharedChunk::allocate`. Both build a `header + payload_size` byte
//! allocation aligned for the chunk header, then write fields through a
//! freshly-constructed fat DST pointer.

use core::alloc::Layout;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

/// Allocate a `header + payload_size` byte allocation aligned to
/// `header_align`.
///
/// Returns `(raw_u8_ptr, layout)` on success. The pointer carries
/// provenance over the full allocation and is suitable as the data field
/// of a slice-DST fat pointer with metadata `payload_size`. The layout is
/// the exact one passed to `allocator.allocate`, suitable for a matching
/// `deallocate` call.
///
/// On size-overflow or end-of-address-space overflow, the allocation is
/// freed and `AllocError` is returned.
#[allow(
    clippy::map_err_ignore,
    reason = "LayoutError carries no payload; only the AllocError variant matters"
)]
#[cfg_attr(test, mutants::skip)]
#[inline]
pub(crate) fn alloc_chunk_raw<A: Allocator>(
    allocator: &A,
    header_size: usize,
    header_align: usize,
    payload_size: usize,
) -> Result<(*mut u8, Layout), AllocError> {
    let total = header_size.checked_add(payload_size).ok_or(AllocError)?;
    let layout = Layout::from_size_align(total, header_align).map_err(|_| AllocError)?;
    let raw = allocator.allocate(layout)?;
    let raw_u8_ptr: *mut u8 = raw.cast::<u8>().as_ptr();
    let start_addr = raw_u8_ptr as usize;
    let end_addr = start_addr.checked_add(total).ok_or(AllocError)?;
    if end_addr > isize::MAX as usize {
        // SAFETY: matches the `allocator.allocate` pair; nothing has
        // been stored in the allocation yet.
        unsafe {
            allocator.deallocate(NonNull::new_unchecked(raw_u8_ptr), layout);
        }
        return Err(AllocError);
    }
    Ok((raw_u8_ptr, layout))
}
