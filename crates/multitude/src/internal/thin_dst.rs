// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generic thin-pointer DST storage helpers shared by [`Arc<T>`] /
//! [`Box<T>`] for arbitrary `T: ?Sized + Pointee`.
//!
//! Layout of every chunk-resident smart-pointer value:
//!
//! ```text
//! [optional pad to align(T)][T::Metadata (unaligned)][T payload]
//! ```
//!
//! Thin smart pointers store `NonNull<u8>` to the payload. Metadata sits
//! immediately before it and is read with [`ptr::read_unaligned`].

use core::mem;
use core::ptr::{self, NonNull};
use core::sync::atomic::AtomicU32;

use ptr_meta::Pointee;

/// Byte size of `T`'s pointer metadata.
///
/// `0` for `T: Sized` (whose `Metadata = ()`); typically
/// `size_of::<usize>()` for slice DSTs and trait objects on 64-bit.
#[inline]
pub(crate) const fn meta_bytes<T: ?Sized + Pointee>() -> usize {
    mem::size_of::<<T as Pointee>::Metadata>()
}

/// Byte size of the per-[`Arc`](crate::Arc) strong reference count
/// (an [`AtomicU32`]) stored in the chunk prefix.
pub(crate) const STRONG_BYTES: usize = mem::size_of::<AtomicU32>();

/// Alignment of the per-`Arc` strong reference count.
pub(crate) const STRONG_ALIGN: usize = mem::align_of::<AtomicU32>();

/// Byte distance from an `Arc<T>` value pointer back to its strong
/// reference count, given the value's alignment and metadata width.
///
/// Layout of every chunk-resident `Arc<T>` value:
///
/// ```text
/// [strong (AtomicU32, at reservation base)][pad][T::Metadata (unaligned)][T payload]
/// ```
///
/// The strong count starts the reservation; metadata sits immediately before
/// the payload. The returned prefix keeps the payload `value_align`-aligned.
#[inline]
pub(crate) const fn strong_prefix_bytes_for(value_align: usize, meta: usize) -> usize {
    (STRONG_BYTES + meta).next_multiple_of(value_align)
}

/// Reservation alignment for an `Arc<T>` value: at least [`STRONG_ALIGN`] and
/// at least `value_align`.
#[inline]
pub(crate) const fn arc_block_align(value_align: usize) -> usize {
    if value_align >= STRONG_ALIGN { value_align } else { STRONG_ALIGN }
}

/// Recovers the strong reference count of an `Arc<T>` from its value
/// pointer.
///
/// # Safety
///
/// - `value_ptr` must reference the payload of an `Arc<T>` value whose
///   chunk prefix was written by the strong-prefixed allocator path.
/// - `value_align` must equal the value's alignment (`align_of_val`).
/// - The hosting chunk must be kept alive by the caller for the
///   duration of the returned reference's use.
#[inline]
pub(crate) unsafe fn strong_ref<'a, T: ?Sized + Pointee>(value_ptr: NonNull<u8>, value_align: usize) -> &'a AtomicU32 {
    let prefix = strong_prefix_bytes_for(value_align, meta_bytes::<T>());
    // SAFETY: per caller. `prefix` bytes of strong + metadata + padding
    // were reserved before the payload; the strong slot lives at the
    // reservation base, which is `STRONG_ALIGN`-aligned, so the
    // `AtomicU32` reference is well-aligned and within chunk provenance.
    unsafe { value_ptr.byte_sub(prefix).cast::<AtomicU32>().as_ref() }
}

/// Reads `T`'s metadata word from the chunk prefix immediately preceding
/// the payload at `value_ptr`.
///
/// # Safety
///
/// - `value_ptr` must point at a fully-initialized `T` whose chunk
///   prefix was written by [`Arena::impl_alloc_thin_smart`].
/// - For `T: Sized` the read is a zero-byte no-op and returns `()`.
#[inline]
pub(crate) unsafe fn read_metadata<T: ?Sized + Pointee>(value_ptr: NonNull<u8>) -> <T as Pointee>::Metadata {
    // SAFETY: per caller. `read_unaligned` works for any element size and
    // alignment; for `T: Sized` (Metadata = ()), this compiles to a no-op
    // returning unit.
    unsafe {
        let meta_ptr = value_ptr.as_ptr().sub(meta_bytes::<T>()).cast::<<T as Pointee>::Metadata>();
        ptr::read_unaligned(meta_ptr)
    }
}

/// Reconstructs a fat `NonNull<T>` from the thin payload pointer by
/// reading metadata from the chunk prefix.
///
/// For `T: Sized`, this is a zero-cost cast (`Metadata = ()`, no read).
///
/// # Safety
///
/// Same as [`read_metadata`].
#[inline]
pub(crate) unsafe fn as_fat<T: ?Sized + Pointee>(value_ptr: NonNull<u8>) -> NonNull<T> {
    // SAFETY: per caller.
    unsafe {
        let meta = read_metadata::<T>(value_ptr);
        let fat = ptr_meta::from_raw_parts_mut::<T>(value_ptr.as_ptr().cast::<()>(), meta);
        NonNull::new_unchecked(fat)
    }
}
