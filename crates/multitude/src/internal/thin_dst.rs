// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Hybrid DST metadata helpers shared by [`Arc<T>`] / [`Rc<T>`] / [`Box<T>`].
//!
//! `Box` allocations with allocation-resident metadata use:
//!
//! ```text
//! [optional pad to align(T)][allocation metadata (unaligned)][T payload]
//! ```
//!
//! `Arc` and `Rc` allocations place a shared strong count first:
//!
//! ```text
//! [strong count][optional pad to align(T)][allocation metadata (unaligned)][T payload]
//! ```
//!
//! Handles always store a `NonNull<u8>` to the payload. Slice-like lengths sit
//! immediately before it and are read with [`ptr::read_unaligned`];
//! trait-object vtable pointers are stored in each handle instead.

use core::mem;
use core::ptr::{self, NonNull};
use core::sync::atomic::AtomicU32;

use allocator_api2::alloc::Allocator;
use ptr_meta::{DynMetadata, Pointee};

/// Selects where pointee metadata is stored.
///
/// This is public only because it participates in the bounds of the public
/// smart-pointer types. It is an implementation detail.
#[doc(hidden)]
#[allow(private_bounds, reason = "sealed implementation-detail bound on public smart pointers")]
pub trait MetadataPolicy<T: ?Sized + Pointee>: Copy + sealed::MetadataSealed<T> {
    /// Metadata carried directly in each smart-pointer handle.
    type Stored: Copy + HandleMetadata<T>;

    /// Bytes reserved immediately before the allocation payload.
    const ALLOCATION_BYTES: usize;

    /// Converts pointer metadata into its per-handle representation.
    fn store(metadata: Self) -> Self::Stored;

    /// Recovers pointer metadata from the allocation and handle.
    ///
    /// # Safety
    ///
    /// `value_ptr` must point to a live value allocated using this policy.
    /// `stored` must describe the allocation's actual concrete pointee type
    /// and layout.
    unsafe fn resolve(value_ptr: NonNull<u8>, stored: Self::Stored) -> Self;

    /// Constructs handle metadata for an allocation-resident metadata value.
    ///
    /// # Panics
    ///
    /// Panics when this policy requires metadata to be supplied by the handle.
    fn from_allocation(value_ptr: NonNull<u8>) -> Self::Stored;
}

/// Sealed per-handle metadata representation used by Multitude smart pointers.
///
/// This is public only because it appears in a hidden defaulted type parameter
/// on the public smart-pointer types.
#[doc(hidden)]
#[allow(private_bounds, reason = "sealed implementation-detail bound on public smart pointers")]
pub trait HandleMetadata<T: ?Sized + Pointee>: Copy + Send + Sync + sealed::HandleMetadataSealed<T> {
    /// Recovers the full pointer metadata.
    ///
    /// # Safety
    ///
    /// `value_ptr` must point to a live allocation whose concrete pointee type
    /// and layout are described by `self`.
    unsafe fn resolve(self, value_ptr: NonNull<u8>) -> T::Metadata;
}

impl<T: ?Sized + Pointee<Metadata = ()>> MetadataPolicy<T> for () {
    type Stored = ();

    const ALLOCATION_BYTES: usize = 0;

    #[inline]
    fn store((): Self) -> Self::Stored {}

    #[inline]
    unsafe fn resolve(_value_ptr: NonNull<u8>, (): Self::Stored) -> Self {}

    #[inline]
    fn from_allocation(_value_ptr: NonNull<u8>) -> Self::Stored {}
}

impl<T: ?Sized + Pointee<Metadata = Self>> MetadataPolicy<T> for usize {
    type Stored = ();

    const ALLOCATION_BYTES: Self = mem::size_of::<Self>();

    #[inline]
    fn store(_metadata: Self) -> Self::Stored {}

    #[inline]
    unsafe fn resolve(value_ptr: NonNull<u8>, (): Self::Stored) -> Self {
        // SAFETY: the policy reserves one unaligned `usize` immediately before
        // the payload and the caller guarantees a live allocation.
        unsafe { ptr::read_unaligned(value_ptr.as_ptr().sub(mem::size_of::<Self>()).cast::<Self>()) }
    }

    #[inline]
    fn from_allocation(_value_ptr: NonNull<u8>) -> Self::Stored {}
}

impl<T, D> MetadataPolicy<T> for DynMetadata<D>
where
    T: ?Sized + Pointee<Metadata = Self>,
    D: ?Sized,
{
    type Stored = Self;

    const ALLOCATION_BYTES: usize = 0;

    #[inline]
    fn store(metadata: Self) -> Self::Stored {
        metadata
    }

    #[inline]
    unsafe fn resolve(_value_ptr: NonNull<u8>, stored: Self::Stored) -> Self {
        stored
    }

    #[cold]
    #[inline(never)]
    #[expect(
        clippy::panic,
        reason = "calling this constructor for handle-resident metadata is a programming error"
    )]
    fn from_allocation(_value_ptr: NonNull<u8>) -> Self::Stored {
        panic!("trait-object metadata must be supplied by the smart-pointer constructor")
    }
}

/// Pointee types supported by Multitude's hybrid smart-pointer representation.
///
/// Sized values and `usize`-metadata DSTs keep one-word handles. Trait-object
/// vtable pointers are stored in each handle so differently coerced views can
/// coexist.
#[doc(hidden)]
#[allow(private_bounds, reason = "sealed implementation-detail bound on public smart pointers")]
pub trait SmartPointerPointee: Pointee + sealed::Sealed {
    /// Metadata physically stored in each handle.
    type StoredMetadata: HandleMetadata<Self>;

    /// Bytes of metadata stored in the allocation prefix.
    const ALLOCATION_METADATA_BYTES: usize;

    /// Converts full pointer metadata into handle metadata.
    fn store_metadata(metadata: Self::Metadata) -> Self::StoredMetadata;

    /// Constructs handle metadata when all metadata is allocation-resident.
    ///
    /// # Panics
    ///
    /// Panics when `Self` requires metadata to be supplied by the handle.
    fn metadata_from_allocation(value_ptr: NonNull<u8>) -> Self::StoredMetadata;

    /// Recovers full pointer metadata.
    ///
    /// # Safety
    ///
    /// `value_ptr` must point to a live allocation whose actual concrete
    /// pointee type and layout are described by `stored`.
    unsafe fn resolve_metadata(value_ptr: NonNull<u8>, stored: Self::StoredMetadata) -> Self::Metadata;
}

mod sealed {
    use ptr_meta::{DynMetadata, Pointee};

    use super::MetadataPolicy;

    pub(crate) trait MetadataSealed<T: ?Sized + Pointee> {}

    impl<T: ?Sized + Pointee<Metadata = ()>> MetadataSealed<T> for () {}
    impl<T: ?Sized + Pointee<Metadata = Self>> MetadataSealed<T> for usize {}
    impl<T, D> MetadataSealed<T> for DynMetadata<D>
    where
        T: ?Sized + Pointee<Metadata = Self>,
        D: ?Sized,
    {
    }

    pub(crate) trait Sealed {}

    impl<T: ?Sized + Pointee> Sealed for T where T::Metadata: MetadataPolicy<T> {}

    pub(crate) trait HandleMetadataSealed<T: ?Sized + Pointee> {}

    impl<T: ?Sized + Pointee> HandleMetadataSealed<T> for () where T::Metadata: MetadataPolicy<T, Stored = ()> {}
    impl<T, D> HandleMetadataSealed<T> for DynMetadata<D>
    where
        T: ?Sized + Pointee,
        D: ?Sized,
        T::Metadata: MetadataPolicy<T, Stored = Self>,
    {
    }
}

impl<T: ?Sized + Pointee> SmartPointerPointee for T
where
    T::Metadata: MetadataPolicy<T>,
{
    type StoredMetadata = <Self::Metadata as MetadataPolicy<Self>>::Stored;

    const ALLOCATION_METADATA_BYTES: usize = <T::Metadata as MetadataPolicy<T>>::ALLOCATION_BYTES;

    #[inline]
    fn store_metadata(metadata: Self::Metadata) -> Self::StoredMetadata {
        <T::Metadata as MetadataPolicy<T>>::store(metadata)
    }

    #[inline]
    fn metadata_from_allocation(value_ptr: NonNull<u8>) -> Self::StoredMetadata {
        <T::Metadata as MetadataPolicy<T>>::from_allocation(value_ptr)
    }

    #[inline]
    unsafe fn resolve_metadata(value_ptr: NonNull<u8>, stored: Self::StoredMetadata) -> Self::Metadata {
        // SAFETY: forwarded from the caller.
        unsafe { <T::Metadata as MetadataPolicy<T>>::resolve(value_ptr, stored) }
    }
}

impl<T: ?Sized + Pointee> HandleMetadata<T> for ()
where
    T::Metadata: MetadataPolicy<T, Stored = ()>,
{
    #[inline]
    unsafe fn resolve(self, value_ptr: NonNull<u8>) -> T::Metadata {
        // SAFETY: forwarded from the caller.
        unsafe { <T::Metadata as MetadataPolicy<T>>::resolve(value_ptr, self) }
    }
}

// Dependency contract: `ptr_meta::DynMetadata<D>` is unconditionally
// `Send + Sync`, independently of `D`. The `HandleMetadata` supertraits make
// this impl stop compiling if that upstream guarantee changes; `send_sync`
// also locks the assumption with explicit compile-time assertions.
impl<T, D> HandleMetadata<T> for DynMetadata<D>
where
    T: ?Sized + Pointee,
    D: ?Sized,
    T::Metadata: MetadataPolicy<T, Stored = Self>,
{
    #[inline]
    unsafe fn resolve(self, value_ptr: NonNull<u8>) -> T::Metadata {
        // SAFETY: forwarded from the caller.
        unsafe { <T::Metadata as MetadataPolicy<T>>::resolve(value_ptr, self) }
    }
}

/// Byte size of the shared [`Arc`](crate::Arc) / [`Rc`](crate::Rc) strong
/// reference count stored in the chunk prefix.
const STRONG_BYTES: usize = mem::size_of::<AtomicU32>();

/// Alignment of the shared `Arc` strong reference count.
const STRONG_ALIGN: usize = mem::align_of::<AtomicU32>();

/// Byte distance from an `Arc<T>` / `Rc<T>` value pointer back to its shared
/// strong reference count, given the value's alignment and metadata width.
///
/// Layout of every chunk-resident strong-prefixed value:
///
/// ```text
/// [strong (AtomicU32 or u32, at reservation base)][optional pad][allocation metadata (unaligned)][T payload]
/// ```
///
/// The strong count starts the reservation; any allocation metadata sits
/// immediately before the payload. The returned prefix keeps the payload
/// `value_align`-aligned.
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

/// Policy describing how a thin shared smart pointer's strong reference
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
    type Ptr<T: ?Sized + SmartPointerPointee, A: Allocator + Clone>;

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
    /// [`Self::write_one`]) and any allocation-resident metadata required by
    /// `T`. Handle-resident metadata is not supported by this method; use
    /// `adopt_with_metadata` instead. The caller must have just taken one `+1`
    /// chunk refcount for the new handle family, and `thin` must lie within the
    /// first `CHUNK_ALIGN` bytes of its hosting chunk so chunk recovery by
    /// masking succeeds.
    unsafe fn adopt<T: ?Sized + SmartPointerPointee, A: Allocator + Clone>(thin: NonNull<u8>) -> Self::Ptr<T, A>;

    /// Adopts a payload whose metadata is supplied directly.
    ///
    /// # Safety
    ///
    /// Same as [`Self::adopt`], except `metadata` must be the metadata for the
    /// initialized `T` at `thin`.
    #[cfg(feature = "dst")]
    unsafe fn adopt_with_metadata<T: ?Sized + SmartPointerPointee, A: Allocator + Clone>(
        thin: NonNull<u8>,
        metadata: T::Metadata,
    ) -> Self::Ptr<T, A>;
}

/// Atomic strong-count policy for [`Arc`](crate::Arc).
pub(crate) enum AtomicStrong {}

/// Non-atomic, unaligned strong-count policy for [`Rc`](crate::Rc).
pub(crate) enum LocalStrong {}

impl Strong for AtomicStrong {
    type Ptr<T: ?Sized + SmartPointerPointee, A: Allocator + Clone> = crate::Arc<T, A>;

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
    unsafe fn adopt<T: ?Sized + SmartPointerPointee, A: Allocator + Clone>(thin: NonNull<u8>) -> crate::Arc<T, A> {
        // SAFETY: `Arc::from_raw` requires exactly what this method's contract
        // demands of `thin` — an initialized payload, an atomic strong count of
        // 1 in the prefix, a held +1 chunk refcount, and an in-first-tile
        // address — so the caller's guarantee discharges it.
        unsafe { crate::Arc::from_raw(thin) }
    }

    #[inline]
    #[cfg(feature = "dst")]
    unsafe fn adopt_with_metadata<T: ?Sized + SmartPointerPointee, A: Allocator + Clone>(
        thin: NonNull<u8>,
        metadata: T::Metadata,
    ) -> crate::Arc<T, A> {
        // SAFETY: forwarded from the caller.
        unsafe { crate::Arc::from_raw_with_metadata(thin, metadata) }
    }
}

impl Strong for LocalStrong {
    type Ptr<T: ?Sized + SmartPointerPointee, A: Allocator + Clone> = crate::Rc<T, A>;

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
    unsafe fn adopt<T: ?Sized + SmartPointerPointee, A: Allocator + Clone>(thin: NonNull<u8>) -> crate::Rc<T, A> {
        // SAFETY: `Rc::from_raw` requires exactly what this method's contract
        // demands of `thin` — an initialized payload, a non-atomic strong count
        // of 1 in the prefix, a held +1 chunk refcount, and an in-first-tile
        // address — so the caller's guarantee discharges it.
        unsafe { crate::Rc::from_raw(thin) }
    }

    #[inline]
    #[cfg(feature = "dst")]
    unsafe fn adopt_with_metadata<T: ?Sized + SmartPointerPointee, A: Allocator + Clone>(
        thin: NonNull<u8>,
        metadata: T::Metadata,
    ) -> crate::Rc<T, A> {
        // SAFETY: forwarded from the caller.
        unsafe { crate::Rc::from_raw_with_metadata(thin, metadata) }
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
pub(crate) unsafe fn local_strong_ptr<T: ?Sized + SmartPointerPointee>(value_ptr: NonNull<u8>, value_align: usize) -> *mut u32 {
    let prefix = strong_prefix_bytes_for(value_align, T::ALLOCATION_METADATA_BYTES);
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
pub(crate) unsafe fn strong_ref<'a, T: ?Sized + SmartPointerPointee>(value_ptr: NonNull<u8>, value_align: usize) -> &'a AtomicU32 {
    let prefix = strong_prefix_bytes_for(value_align, T::ALLOCATION_METADATA_BYTES);
    // SAFETY: per caller. `prefix` bytes of strong + metadata + padding
    // were reserved before the payload; the strong slot lives at the
    // reservation base, which is `STRONG_ALIGN`-aligned, so the
    // `AtomicU32` reference is well-aligned and within chunk provenance.
    unsafe { value_ptr.byte_sub(prefix).cast::<AtomicU32>().as_ref() }
}

/// Reconstructs a fat `NonNull<T>` from the thin payload pointer by
/// combining allocation-resident and handle-resident metadata.
///
/// For `T: Sized`, this is a zero-cost cast (`Metadata = ()`, no read).
///
/// # Safety
///
/// `value_ptr` and `stored` must describe the same live allocation.
#[inline]
pub(crate) unsafe fn as_fat<T: ?Sized + SmartPointerPointee, M: HandleMetadata<T>>(value_ptr: NonNull<u8>, stored: M) -> NonNull<T> {
    // SAFETY: per caller.
    unsafe {
        let meta = stored.resolve(value_ptr);
        let fat = ptr_meta::from_raw_parts_mut::<T>(value_ptr.as_ptr().cast::<()>(), meta);
        NonNull::new_unchecked(fat)
    }
}
