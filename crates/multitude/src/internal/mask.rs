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

/// Chunk-header types that share the masking-based recovery layout:
/// they carry a `capacity: usize` field at the sized prefix and are
/// the metadata target of a `*mut [Self]` fat pointer of length
/// `capacity`.
///
/// # Safety
///
/// Implementors must:
/// * begin with a sized prefix whose `capacity` field gives the byte
///   length of the trailing payload (the fat-pointer metadata);
/// * be reachable via the chunk-header invariant (start of every
///   `CHUNK_ALIGN`-tile owned by the allocator).
pub(crate) unsafe trait ChunkHeader {
    /// Reconstruct a fat-pointer `NonNull<Self>` from a header-byte
    /// pointer by reading the `capacity` field as the metadata.
    ///
    /// # Safety
    ///
    /// `header_byte_ptr` must point at a live header's sized prefix
    /// (start of a `CHUNK_ALIGN` tile owned by the allocator).
    unsafe fn fat_from_header_bytes(header_byte_ptr: *mut u8) -> NonNull<Self>;
}

// SAFETY: `LocalChunk<A>` begins with a `capacity: usize` field and
// the chunk-header invariant places it at a `CHUNK_ALIGN`-tile start.
unsafe impl<A: Allocator + Clone> ChunkHeader for LocalChunk<A> {
    #[inline]
    unsafe fn fat_from_header_bytes(header_byte_ptr: *mut u8) -> NonNull<Self> {
        // SAFETY: chunk-header invariant — header prefix is live and
        // initialized, so reading `capacity` and reconstituting the
        // fat pointer is sound.
        unsafe {
            let header_only: *const Self = core::ptr::slice_from_raw_parts(header_byte_ptr, 0) as *const Self;
            let capacity = (*header_only).capacity;
            let fat: *mut Self = core::ptr::slice_from_raw_parts_mut(header_byte_ptr, capacity) as *mut Self;
            NonNull::new_unchecked(fat)
        }
    }
}

// SAFETY: same as `LocalChunk` — `capacity: usize` is the sized prefix.
unsafe impl<A: Allocator + Clone> ChunkHeader for SharedChunk<A> {
    #[inline]
    unsafe fn fat_from_header_bytes(header_byte_ptr: *mut u8) -> NonNull<Self> {
        // SAFETY: see `LocalChunk` impl.
        unsafe {
            let header_only: *const Self = core::ptr::slice_from_raw_parts(header_byte_ptr, 0) as *const Self;
            let capacity = (*header_only).capacity;
            let fat: *mut Self = core::ptr::slice_from_raw_parts_mut(header_byte_ptr, capacity) as *mut Self;
            NonNull::new_unchecked(fat)
        }
    }
}

/// Recover the chunk header for `value` by masking within its
/// `CHUNK_ALIGN` tile.
///
/// # Safety
///
/// `value` must come from an arena allocation whose chunk flavor is
/// `C` and still satisfy the chunk-header invariant.
#[inline]
pub(crate) unsafe fn chunk_of<C: ChunkHeader + ?Sized, T: ?Sized>(value: NonNull<T>) -> NonNull<C> {
    let raw = value.as_ptr().cast::<u8>();
    let offset_within_chunk = (raw as usize) & (CHUNK_ALIGN - 1);
    // SAFETY: the chunk-header invariant says this lands on the
    // header, and `byte_sub` preserves provenance; rebuilding the fat
    // pointer is delegated to the per-flavor impl.
    unsafe {
        let header_byte_ptr = raw.byte_sub(offset_within_chunk);
        C::fat_from_header_bytes(header_byte_ptr)
    }
}

/// Recover the [`LocalChunk`] header for `value`.
///
/// # Safety
///
/// `value` must come from an [`Arena`](crate::Arena) allocation and
/// still satisfy the chunk-header invariant.
#[inline]
pub(crate) unsafe fn local_chunk_of<T: ?Sized, A: Allocator + Clone>(value: NonNull<T>) -> NonNull<LocalChunk<A>> {
    // SAFETY: forwarded to caller's contract on `value`.
    unsafe { chunk_of::<LocalChunk<A>, T>(value) }
}

/// Recover the [`SharedChunk`] header for `value`.
///
/// # Safety
///
/// `value` must come from an [`Arena::alloc_arc*`](crate::Arena::alloc_arc)
/// allocation and still satisfy the chunk-header invariant.
#[inline]
pub(crate) unsafe fn shared_chunk_of<T: ?Sized, A: Allocator + Clone>(value: NonNull<T>) -> NonNull<SharedChunk<A>> {
    // SAFETY: forwarded to caller's contract on `value`.
    unsafe { chunk_of::<SharedChunk<A>, T>(value) }
}
