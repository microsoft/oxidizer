// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generic thin-pointer DST storage helpers shared by [`Arc<T>`] / [`Rc<T>`] /
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

use allocator_api2::alloc::Allocator;
use ptr_meta::Pointee;

/// Byte size of `T`'s pointer metadata.
///
/// `0` for `T: Sized` (whose `Metadata = ()`); typically
/// `size_of::<usize>()` for slice DSTs and trait objects on 64-bit.
#[inline]
const fn meta_bytes<T: ?Sized + Pointee>() -> usize {
    mem::size_of::<<T as Pointee>::Metadata>()
}

/// Byte size of the per-[`Arc`](crate::Arc) strong reference count
/// (an [`AtomicU32`]) stored in the chunk prefix.
const STRONG_BYTES: usize = mem::size_of::<AtomicU32>();

/// Alignment of the per-`Arc` strong reference count.
const STRONG_ALIGN: usize = mem::align_of::<AtomicU32>();

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

/// Policy describing how a thin smart pointer's per-handle strong reference
/// count is stored in the chunk prefix.
///
/// [`Arc`](crate::Arc) uses [`AtomicStrong`] (a thread-safe [`AtomicU32`] that
/// must be naturally aligned); [`Rc`](crate::Rc) uses [`LocalStrong`] (a
/// non-atomic `u32` accessed through unaligned loads/stores, so its reservation
/// needs no `STRONG_ALIGN` floor and packs tighter for `str` / `[u8]`).
///
/// The count is always 4 bytes ([`STRONG_BYTES`]); only the reservation
/// alignment and the read/write discipline differ.
pub(crate) trait Strong {
    /// The thin smart-pointer type that adopts allocations made under this
    /// policy: [`Arc`](crate::Arc) for [`AtomicStrong`], [`Rc`](crate::Rc) for
    /// [`LocalStrong`]. Lets the shared allocation helpers return the finished
    /// handle directly, so the conversion from a raw payload pointer lives in
    /// exactly one place ([`Self::adopt`]) rather than at every call site.
    type Ptr<T: ?Sized + Pointee, A: Allocator + Clone>;

    /// Reservation block alignment given the value's alignment.
    fn block_align(value_align: usize) -> usize;

    /// Writes the initial strong count (`1`) at the reservation base.
    ///
    /// # Safety
    ///
    /// `base` must address [`STRONG_BYTES`] writable bytes at the start of a
    /// reservation aligned to [`Self::block_align`].
    unsafe fn write_one(base: *mut u8);

    /// Adopts a freshly bump-allocated thin payload pointer into this policy's
    /// smart pointer, taking ownership of the value and the family's chunk
    /// reference.
    ///
    /// # Safety
    ///
    /// `thin` must point at the payload of a fully-initialized `T` whose chunk
    /// prefix holds a strong count already initialized to `1` (via
    /// [`Self::write_one`]) and, for DST `T`, the matching `T::Metadata`
    /// immediately before the payload. The caller must have just taken one `+1`
    /// chunk refcount for the new handle family, and `thin` must lie within the
    /// first `CHUNK_ALIGN` bytes of its hosting chunk so chunk recovery by
    /// masking succeeds.
    unsafe fn adopt<T: ?Sized + Pointee, A: Allocator + Clone>(thin: NonNull<u8>) -> Self::Ptr<T, A>;
}

/// Atomic strong-count policy for [`Arc`](crate::Arc).
pub(crate) enum AtomicStrong {}

/// Non-atomic, unaligned strong-count policy for [`Rc`](crate::Rc).
pub(crate) enum LocalStrong {}

impl Strong for AtomicStrong {
    type Ptr<T: ?Sized + Pointee, A: Allocator + Clone> = crate::Arc<T, A>;

    #[inline]
    fn block_align(value_align: usize) -> usize {
        arc_block_align(value_align)
    }

    #[inline]
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "block_align floors at STRONG_ALIGN, so `base` is aligned for AtomicU32"
    )]
    unsafe fn write_one(base: *mut u8) {
        // SAFETY: per the contract, `base` is `STRONG_ALIGN`-aligned.
        unsafe { base.cast::<AtomicU32>().write(AtomicU32::new(1)) };
    }

    #[inline]
    unsafe fn adopt<T: ?Sized + Pointee, A: Allocator + Clone>(thin: NonNull<u8>) -> crate::Arc<T, A> {
        // SAFETY: `Arc::from_raw` requires exactly what this method's contract
        // demands of `thin` — an initialized payload, an atomic strong count of
        // 1 in the prefix, a held +1 chunk refcount, and an in-first-tile
        // address — so the caller's guarantee discharges it.
        unsafe { crate::Arc::from_raw(thin) }
    }
}

impl Strong for LocalStrong {
    type Ptr<T: ?Sized + Pointee, A: Allocator + Clone> = crate::Rc<T, A>;

    #[inline]
    fn block_align(value_align: usize) -> usize {
        // No atomic alignment floor: a non-atomic count may be unaligned.
        value_align
    }

    #[inline]
    unsafe fn write_one(base: *mut u8) {
        // SAFETY: `base` addresses STRONG_BYTES writable bytes; the count is
        // non-atomic, so an unaligned store is sound.
        unsafe { ptr::write_unaligned(base.cast::<u32>(), 1) };
    }

    #[inline]
    unsafe fn adopt<T: ?Sized + Pointee, A: Allocator + Clone>(thin: NonNull<u8>) -> crate::Rc<T, A> {
        // SAFETY: `Rc::from_raw` requires exactly what this method's contract
        // demands of `thin` — an initialized payload, a non-atomic strong count
        // of 1 in the prefix, a held +1 chunk refcount, and an in-first-tile
        // address — so the caller's guarantee discharges it.
        unsafe { crate::Rc::from_raw(thin) }
    }
}

/// Recovers a raw pointer to an [`Rc`](crate::Rc)'s non-atomic, possibly
/// unaligned strong reference count from its value pointer.
///
/// The count must be accessed only with [`ptr::read_unaligned`] /
/// [`ptr::write_unaligned`] — never by forming a `&u32`, which would be
/// undefined behavior at a misaligned address.
///
/// # Safety
///
/// Same contract as [`strong_ref`], for a value allocated through the
/// [`LocalStrong`] path.
#[inline]
pub(crate) unsafe fn local_strong_ptr<T: ?Sized + Pointee>(value_ptr: NonNull<u8>, value_align: usize) -> *mut u32 {
    let prefix = strong_prefix_bytes_for(value_align, meta_bytes::<T>());
    // SAFETY: per caller; `prefix` bytes of strong + metadata + padding were
    // reserved before the payload, and the count lives at the reservation base.
    unsafe { value_ptr.byte_sub(prefix).cast::<u32>().as_ptr() }
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
unsafe fn read_metadata<T: ?Sized + Pointee>(value_ptr: NonNull<u8>) -> <T as Pointee>::Metadata {
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
