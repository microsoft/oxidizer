// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Recover chunk headers from value pointers by masking within the
//! `CHUNK_ALIGN` tile.
//!
//! We use `byte_sub` so the recovered header keeps the original
//! pointer's provenance; rebuilding from an integer would fail under
//! strict provenance.

use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::constants::CHUNK_ALIGN;
use super::local_chunk::LocalChunk;
use super::shared_chunk::SharedChunk;

/// Recover the [`LocalChunk`] header for `value`.
///
/// # Safety
///
/// `value` must come from an [`Arena`](crate::Arena) allocation and
/// still satisfy the chunk-header invariant.
#[inline]
pub(crate) unsafe fn local_chunk_of<T: ?Sized, A: Allocator + Clone>(value: NonNull<T>) -> NonNull<LocalChunk<A>> {
    let raw = value.as_ptr().cast::<u8>();
    let offset_within_chunk = (raw as usize) & (CHUNK_ALIGN - 1);
    // SAFETY: the chunk-header invariant says this lands on the
    // header, and `byte_sub` preserves provenance.
    let header_byte_ptr = unsafe { raw.byte_sub(offset_within_chunk) };

    // Read the sized prefix so we can rebuild the fat chunk pointer.
    let header_only: *const LocalChunk<A> = core::ptr::slice_from_raw_parts(header_byte_ptr, 0) as *const LocalChunk<A>;
    // SAFETY: chunk-header invariant — header prefix is live and initialized.
    let capacity = unsafe { (*header_only).capacity };

    let fat: *mut LocalChunk<A> = core::ptr::slice_from_raw_parts_mut(header_byte_ptr, capacity) as *mut LocalChunk<A>;
    // SAFETY: `value` is non-null, and `byte_sub` stays in the same allocation.
    unsafe { NonNull::new_unchecked(fat) }
}

/// Recover the [`SharedChunk`] header for `value`.
///
/// # Safety
///
/// `value` must come from an [`Arena::alloc_arc*`](crate::Arena::alloc_arc)
/// allocation and still satisfy the chunk-header invariant.
#[inline]
pub(crate) unsafe fn shared_chunk_of<T: ?Sized, A: Allocator + Clone>(value: NonNull<T>) -> NonNull<SharedChunk<A>> {
    let raw = value.as_ptr().cast::<u8>();
    let offset_within_chunk = (raw as usize) & (CHUNK_ALIGN - 1);
    // SAFETY: same rationale as `local_chunk_of`.
    let header_byte_ptr = unsafe { raw.byte_sub(offset_within_chunk) };

    let header_only: *const SharedChunk<A> = core::ptr::slice_from_raw_parts(header_byte_ptr, 0) as *const SharedChunk<A>;
    // SAFETY: chunk-header invariant — header prefix is live and initialized.
    let capacity = unsafe { (*header_only).capacity };

    let fat: *mut SharedChunk<A> = core::ptr::slice_from_raw_parts_mut(header_byte_ptr, capacity) as *mut SharedChunk<A>;
    // SAFETY: `header_byte_ptr` is non-null (see `local_chunk_of`).
    unsafe { NonNull::new_unchecked(fat) }
}
