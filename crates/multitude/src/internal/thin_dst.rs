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
//! The thin smart pointer stores a `NonNull<u8>` to the payload start.
//! Metadata (slice length, trait-object vtable, or `()` for sized T)
//! sits in `size_of::<T::Metadata>()` bytes immediately preceding the
//! payload and is read with [`ptr::read_unaligned`]. For
//! `T: Sized`, the metadata read is a zero-byte no-op.

use core::mem;
use core::ptr::{self, NonNull};

use ptr_meta::Pointee;

/// Byte size of `T`'s pointer metadata.
///
/// `0` for `T: Sized` (whose `Metadata = ()`); typically
/// `size_of::<usize>()` for slice DSTs and trait objects on 64-bit.
#[inline]
pub(crate) const fn meta_bytes<T: ?Sized + Pointee>() -> usize {
    mem::size_of::<<T as Pointee>::Metadata>()
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
