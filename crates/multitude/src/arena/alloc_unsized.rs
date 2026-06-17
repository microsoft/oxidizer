// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! DST (unsized) value allocation API on [`Arena`].
//!
//! Implements `alloc_dst_arc`, `alloc_dst_box` and their `try_*`
//! variants under the `dst` Cargo feature. The pointer-metadata is
//! stored verbatim in the chunk prefix (immediately before the
//! payload), so supported DSTs are those whose metadata is either
//! zero-sized (sized `T`) or `usize`-sized (slice DSTs and trait
//! objects). `Arc` runs `T`'s destructor eagerly on the last clone via
//! `drop_in_place::<T>`; `Box` does so in its own `Drop`.
//!
//! The legacy `Box` path additionally rejects `T: Drop` DSTs whose
//! metadata does not pack into a `u16` (preserved for parity); the
//! `Arc` path has no such restriction.

use core::alloc::Layout;
use core::mem;
use core::pin::Pin;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{AllocError, Allocator};
use ptr_meta::Pointee;

use super::alloc_value::acquire_shared_chunk_ref;
use super::{Arena, ExpectAlloc};
use crate::arc::Arc;
use crate::r#box::Box;
use crate::internal::constants::max_smart_ptr_align;

/// Maximum `layout.align()` accepted by smart-pointer allocations.
/// Mirrors the constant of the same name in [`alloc_value`](super::alloc_value):
/// values must lie strictly inside the first `CHUNK_ALIGN` bytes of
/// their chunk so the header-recovery mask works.
const MAX_SMART_PTR_ALIGN: usize = max_smart_ptr_align();

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate a possibly-unsized `T` and return an `Arc<T, A>`.
    ///
    /// The closure `init` receives a typed fat pointer to the buffer
    /// (built from `(thin_ptr, metadata)`) and is responsible for
    /// writing a valid `T` through it. The metadata is stored in the
    /// chunk prefix and recovered on demand, so `T`'s destructor runs
    /// eagerly (via `drop_in_place::<T>`) when the last `Arc` clone is
    /// dropped.
    ///
    /// For sized `T`, prefer [`Self::alloc_arc`] / [`Self::alloc_arc_with`].
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `layout.align()` is
    /// at least 32 KiB.
    ///
    /// # Safety
    ///
    /// - `layout` must exactly describe the size and alignment of the
    ///   constructed DST value (e.g., for `[U]` of length `n`,
    ///   `Layout::array::<U>(n).unwrap()`). Passing a smaller layout
    ///   would cause `init` to write past the reservation.
    /// - `init` must initialize all bytes covered by `layout` to a valid `T`.
    /// - `metadata` must be valid for the value just written.
    /// - `T::Metadata` must be either zero-sized (sized `T`) or
    ///   `usize`-sized (slice DSTs `[U]` and trait objects `dyn Trait`,
    ///   whose metadata — slice length or vtable pointer — is stored
    ///   verbatim in the chunk prefix).
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn alloc_dst_arc<T: ?Sized + Send + Sync + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Arc<T, A>
    where
        A: Send + Sync,
    {
        // SAFETY: forwarded — caller's contract on `layout`/`metadata`/`init`.
        unsafe { self.impl_alloc_dst_arc::<T>(layout, metadata, init) }.expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_dst_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `layout.align()` is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// Allocator panics propagate.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_arc<T: ?Sized + Send + Sync + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Arc<T, A>, AllocError>
    where
        A: Send + Sync,
    {
        // SAFETY: forwarded.
        unsafe { self.impl_alloc_dst_arc::<T>(layout, metadata, init) }
    }

    /// Allocate a possibly-unsized `T` and return a [`Box<T, A>`](crate::Box).
    /// See [`Self::alloc_dst_arc`] for the contract.
    ///
    /// Unlike the refcount variants, the resulting [`Box`](crate::Box) runs
    /// `T`'s destructor immediately when the smart pointer is dropped.
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// See [`Self::alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn alloc_dst_box<T: ?Sized + Pointee>(&self, layout: Layout, metadata: T::Metadata, init: impl FnOnce(*mut T)) -> Box<T, A> {
        // SAFETY: forwarded.
        unsafe { self.impl_alloc_dst_box::<T>(layout, metadata, init) }.expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_dst_box`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_arc`].
    ///
    /// # Panics
    ///
    /// Allocator panics propagate.
    ///
    /// # Safety
    ///
    /// See [`Self::alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_box<T: ?Sized + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Box<T, A>, AllocError> {
        // SAFETY: forwarded.
        unsafe { self.impl_alloc_dst_box::<T>(layout, metadata, init) }
    }

    /// Shared implementation for `alloc_dst_arc` / `try_alloc_dst_arc`.
    ///
    /// Reserves `layout.size()` bytes aligned to `layout.align()` in
    /// the current shared chunk, places a drop-entry placeholder (if
    /// `T` requires drop), invokes `init` on the typed fat pointer,
    /// commits the drop shim, and wraps the result in an [`Arc`].
    ///
    /// `TRY` selects the panic / error arm.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    #[inline]
    unsafe fn impl_alloc_dst_arc<T: ?Sized + Send + Sync + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Arc<T, A>, AllocError>
    where
        A: Send + Sync,
    {
        // SAFETY: forwarded.
        let thin = unsafe { self.impl_alloc_dst_smart::<T>(layout, metadata, init) }?;
        // SAFETY: `impl_alloc_dst_smart` returns a thin payload pointer
        // into a chunk whose prefix carries `T::Metadata` and that
        // holds a fresh +1 in the new `Arc`'s name.
        Ok(unsafe { Arc::from_raw(thin) })
    }

    /// Shared implementation for `alloc_dst_box` / `try_alloc_dst_box`.
    /// Mirrors `impl_alloc_dst_arc` but skips drop-entry reservation:
    /// [`Box::drop`] runs `drop_in_place::<T>` on the value pointer
    /// (which natively handles `?Sized`), so no chunk-teardown drop
    /// entry is needed.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_box`].
    #[inline]
    unsafe fn impl_alloc_dst_box<T: ?Sized + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Box<T, A>, AllocError> {
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            return Err(AllocError);
        }
        // Guard parity with the Arc path: even though `Box::drop` runs
        // `T::drop` eagerly (no chunk-teardown drop entry needed), reject
        // DST values with `T: Drop` whose metadata cannot pack into the
        // chunk drop-list's `u16` slot. This keeps the Box convertible
        // to `Arc<T, A>` later via `into_arc`-style APIs and matches the
        // non-DST `alloc_slice_box` family.
        if mem::needs_drop::<T>() && !metadata_fits_u16::<T>(metadata) {
            return Err(AllocError);
        }
        let meta_bytes = mem::size_of::<T::Metadata>();
        // Payload starts at the lowest layout-aligned offset >=
        // meta_bytes. For sized T (meta_bytes = 0) payload starts at 0.
        let payload_offset = if meta_bytes == 0 { 0 } else { meta_bytes.max(layout.align()) };
        // Floor the value byte count to 1 so the returned payload pointer
        // (at offset `payload_offset` within the reservation) is strictly
        // less than `reservation_end`, never landing at
        // `chunk_base + CHUNK_ALIGN` for `layout.size() == 0`.
        let value_bytes = layout.size().max(1);
        let total = payload_offset.checked_add(value_bytes).ok_or(AllocError)?;
        // Refill hint must include `layout.align() - 1` bytes of slack
        // so `try_alloc(total, align)` always succeeds inside a chunk
        // sized for this allocation. The same hint drives the oversized
        // routing check so the dedicated chunk also has the slack.
        let refill_hint = total.saturating_add(layout.align());
        let mut init = Some(init);
        loop {
            if let Some((reservation, chunk_ptr)) = self.current_shared().try_alloc_with_chunk(total, layout.align().max(1)) {
                let init = init.take().expect("init taken twice");
                let chunk_ref = self.acquire_current_shared_chunk_ref(chunk_ptr);
                // SAFETY: see `write_dst_prefix_and_init` — `reservation`
                // is the freshly reserved exclusive storage; we write
                // metadata at `payload - meta_bytes` and hand `init` a
                // fat pointer to the payload.
                let payload_nn =
                    unsafe { write_dst_prefix_and_init::<T>(reservation.as_non_null(), payload_offset, meta_bytes, metadata, init) };
                let _ = chunk_ref.forget();
                // SAFETY: `payload_nn` references a fully-initialized
                // `T` whose metadata is in the chunk prefix; the
                // hosting chunk now holds +1 in the new `Box`'s name.
                return Ok(unsafe { Box::from_raw(payload_nn) });
            }
            if self.is_oversized(refill_hint) {
                let init = init.take().expect("init taken twice");
                return self.alloc_oversized_shared_with(refill_hint, |mutator, chunk_ptr| {
                    let (reservation, _chunk) = mutator
                        .try_alloc_with_chunk(total, layout.align().max(1))
                        .expect("dedicated oversized chunk sized to fit DST value + alignment slack");
                    let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                    // SAFETY: see the in-arena branch above.
                    let payload_nn =
                        unsafe { write_dst_prefix_and_init::<T>(reservation.as_non_null(), payload_offset, meta_bytes, metadata, init) };
                    let _ = chunk_ref.forget();
                    // SAFETY: see the in-arena branch above.
                    unsafe { Box::from_raw(payload_nn) }
                });
            }
            self.refill_shared(refill_hint)?;
        }
    }

    /// Reserve a strong-prefixed `Arc<T>` slot in the current shared
    /// chunk (per-`Arc` strong count + `T::Metadata` prefix + payload),
    /// run `init` on a typed fat pointer, and return the thin payload
    /// pointer. No chunk drop entry is reserved:
    /// [`Arc::drop`](crate::Arc) runs `drop_in_place::<T>` (which natively
    /// handles `?Sized`) on the last reference.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    #[inline]
    unsafe fn impl_alloc_dst_smart<T: ?Sized + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<NonNull<u8>, AllocError> {
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            return Err(AllocError);
        }
        let meta_bytes = mem::size_of::<T::Metadata>();
        let value_align = layout.align().max(1);
        // Floor the value byte count to 1 so the returned payload pointer
        // is strictly inside the reservation (mask-recovery invariant).
        let payload_bytes = layout.size().max(1);
        let refill_hint = worst_case_arc_dst(payload_bytes, value_align, meta_bytes);

        let mut init = Some(init);
        loop {
            if let Some((value_ptr, chunk_ptr)) = self.current_shared().try_alloc_arc_dst(payload_bytes, value_align, meta_bytes) {
                let init = init.take().expect("init taken twice");
                let chunk_ref = self.acquire_current_shared_chunk_ref(chunk_ptr);
                // SAFETY: `value_ptr` is the freshly reserved payload
                // pointer (strong prefix written, count = 1); we write
                // metadata at `value_ptr - meta_bytes` and hand `init` a
                // fat pointer.
                let payload_nn = unsafe { write_dst_meta_and_init::<T>(value_ptr, meta_bytes, metadata, init) };
                let _ = chunk_ref.forget();
                return Ok(payload_nn);
            }

            if self.is_oversized(refill_hint) {
                let init = init.take().expect("init taken twice");
                return self.alloc_oversized_shared_with(refill_hint, |mutator, chunk_ptr| {
                    let (value_ptr, _chunk) = mutator
                        .try_alloc_arc_dst(payload_bytes, value_align, meta_bytes)
                        .expect("dedicated oversized chunk sized to fit DST value + strong prefix");
                    let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                    // SAFETY: see the in-arena branch above.
                    let payload_nn = unsafe { write_dst_meta_and_init::<T>(value_ptr, meta_bytes, metadata, init) };
                    let _ = chunk_ref.forget();
                    payload_nn
                });
            }
            self.refill_shared(refill_hint)?;
        }
    }
}

#[cfg(feature = "dst")]
impl<A: Allocator + Clone> Arena<A> {
    /// `Pin` variant of [`Self::alloc_dst_arc`]. Returns a pinned
    /// `Arc<T, A>` where the value's address is fixed in the arena
    /// and never moves until the last `Arc` clone is dropped.
    ///
    /// Typical use: pinning an `Arc<[T]>` whose slice contents must
    /// stay at a fixed address (e.g. for `Pin`-projecting code).
    /// Trait objects whose metadata is a vtable pointer are **not**
    /// supported (see [`Self::try_alloc_dst_arc`]'s safety contract).
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    #[must_use]
    pub unsafe fn alloc_dst_arc_pin<T: ?Sized + Send + Sync + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Pin<Arc<T, A>>
    where
        A: Send + Sync + 'static,
    {
        // SAFETY: forwarded.
        Arc::into_pin(unsafe { self.alloc_dst_arc::<T>(layout, metadata, init) })
    }

    /// Fallible variant of [`Self::alloc_dst_arc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::try_alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_arc_pin<T: ?Sized + Send + Sync + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Pin<Arc<T, A>>, AllocError>
    where
        A: Send + Sync + 'static,
    {
        // SAFETY: forwarded.
        unsafe { self.try_alloc_dst_arc::<T>(layout, metadata, init) }.map(Arc::into_pin)
    }

    /// `Pin` variant of [`Self::alloc_dst_box`]. Trait objects are
    /// **not** supported (see [`Self::try_alloc_dst_arc`]'s safety
    /// contract); use the slice or sized variants.
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_box`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_box`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    #[must_use]
    pub unsafe fn alloc_dst_box_pin<T: ?Sized + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Pin<Box<T, A>>
    where
        A: 'static,
    {
        // SAFETY: forwarded.
        Box::into_pin(unsafe { self.alloc_dst_box::<T>(layout, metadata, init) })
    }

    /// Fallible variant of [`Self::alloc_dst_box_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_box`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::try_alloc_dst_box`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_box_pin<T: ?Sized + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Pin<Box<T, A>>, AllocError>
    where
        A: 'static,
    {
        // SAFETY: forwarded.
        unsafe { self.try_alloc_dst_box::<T>(layout, metadata, init) }.map(Box::into_pin)
    }
}

/// Returns whether `metadata` packs losslessly into a `u16`.
#[cfg_attr(test, mutants::skip)] // see `alloc_slice_ref::reject_drop_slice_too_long`
#[inline]
fn metadata_fits_u16<T: ?Sized + Pointee>(metadata: T::Metadata) -> bool {
    if mem::size_of::<T::Metadata>() == 0 {
        return true;
    }
    if mem::size_of::<T::Metadata>() != mem::size_of::<usize>() {
        return false;
    }
    // SAFETY: branch above ensures `T::Metadata` is `usize`-sized.
    let raw: usize = unsafe { mem::transmute_copy::<T::Metadata, usize>(&metadata) };
    u16::try_from(raw).is_ok()
}

/// Worst-case byte budget for a single strong-prefixed `Arc<T>` DST
/// allocation: per-`Arc` strong count + `T::Metadata` prefix + payload +
/// front alignment slack.
#[cfg_attr(test, mutants::skip)] // underestimating refill hint ⇒ refill spin
#[inline]
fn worst_case_arc_dst(payload_bytes: usize, value_align: usize, meta_bytes: usize) -> usize {
    use crate::internal::thin_dst;
    thin_dst::strong_prefix_bytes_for(value_align, meta_bytes)
        .saturating_add(payload_bytes)
        .saturating_add(thin_dst::arc_block_align(value_align))
}

/// Write `T::Metadata` (if any) at `value_ptr - meta_bytes`, reconstruct
/// the fat `*mut T`, run the caller-provided `init` on it, and return
/// the thin payload pointer. Used by the strong-prefixed `Arc<T>` DST
/// path, where the strong count already sits before the metadata.
///
/// # Safety
///
/// - `value_ptr` must be the payload pointer of a strong-prefixed `Arc`
///   reservation whose prefix has room for `meta_bytes` immediately
///   before it.
/// - `init` must initialize a valid `T` through the fat pointer it
///   receives.
#[inline(always)]
unsafe fn write_dst_meta_and_init<T: ?Sized + Pointee>(
    value_ptr: NonNull<u8>,
    meta_bytes: usize,
    metadata: T::Metadata,
    init: impl FnOnce(*mut T),
) -> NonNull<u8> {
    // SAFETY: per the function contract. The metadata word sits in
    // `[value_ptr - meta_bytes, value_ptr)`, inside the reservation
    // prefix; `write_unaligned` tolerates any alignment. For sized T
    // (meta_bytes == 0) the write is skipped.
    let fat = unsafe {
        if meta_bytes != 0 {
            let prefix_ptr = value_ptr.as_ptr().sub(meta_bytes).cast::<T::Metadata>();
            ptr::write_unaligned(prefix_ptr, metadata);
        }
        ptr_meta::from_raw_parts_mut::<T>(value_ptr.as_ptr().cast::<()>(), metadata)
    };
    // Caller's contract: `init` writes a valid `T` through `fat`. If it
    // panics, callers' `ChunkRef` guard releases the chunk's `+1`.
    init(fat);
    value_ptr
}

/// `Box` DST variant of [`write_dst_meta_and_init`]: takes the
/// reservation `base` and a `payload_offset`, writes `T::Metadata` at
/// `base + payload_offset - meta_bytes`, runs `init` on the fat
/// pointer, and returns the thin payload pointer
/// (`base + payload_offset`). `Box` has no strong-count prefix, so its
/// reservation starts at the metadata region rather than the strong
/// slot.
///
/// # Safety
///
/// - `base` must reference `payload_offset + layout.size()` bytes of
///   exclusively-owned chunk storage aligned to `layout.align()`.
/// - `payload_offset` must equal `meta_bytes.max(layout.align())` for
///   DST or `0` for sized `T`.
/// - `init` must initialize a valid `T` through the fat pointer.
#[inline(always)]
unsafe fn write_dst_prefix_and_init<T: ?Sized + Pointee>(
    base: NonNull<u8>,
    payload_offset: usize,
    meta_bytes: usize,
    metadata: T::Metadata,
    init: impl FnOnce(*mut T),
) -> NonNull<u8> {
    // SAFETY: per the function contract. `byte_add(payload_offset)`
    // stays within the reservation. The prefix at `payload - meta_bytes`
    // lies in `[base, base + payload_offset)`. For sized T (meta_bytes
    // == 0) the prefix write is a no-op.
    let (payload_nn, fat) = unsafe {
        let payload_nn = base.byte_add(payload_offset);
        if meta_bytes != 0 {
            let prefix_ptr = payload_nn.as_ptr().sub(meta_bytes).cast::<T::Metadata>();
            ptr::write_unaligned(prefix_ptr, metadata);
        }
        let fat = ptr_meta::from_raw_parts_mut::<T>(payload_nn.as_ptr().cast::<()>(), metadata);
        (payload_nn, fat)
    };
    // Caller's contract: `init` writes a valid `T` through `fat`. If it
    // panics, callers' `ChunkRef` guard releases the chunk's `+1`.
    init(fat);
    payload_nn
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Arena as TestArena;

    /// Exercises `alloc_dst_arc` of a sized drop-bearing `T`: the value's
    /// destructor must run eagerly when the last `Arc` clone drops
    /// (before the arena is torn down).
    #[test]
    fn dst_arc_sized_drop_type_metadata_zero_sized_paths() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct D(StdArc<AtomicUsize>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let counter = StdArc::new(AtomicUsize::new(0));
        let counter_for_init = StdArc::clone(&counter);
        let arena: TestArena = TestArena::new();
        let layout = Layout::new::<D>();
        // SAFETY: `init` writes a valid `D` through `ptr`.
        let h: Arc<D> = unsafe {
            arena.alloc_dst_arc::<D>(layout, (), move |p: *mut D| {
                p.write(D(counter_for_init));
            })
        };
        drop(h);
        drop(arena);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    // A `?Sized` type whose `ptr_meta` pointer metadata (`u8`) is neither
    // zero-sized (as for `Sized` `T`) nor `usize`-sized (as for slices, `str`,
    // and trait objects). No DST produced by real allocations has such
    // metadata, so this exercises the otherwise-unreachable reject branch in
    // `metadata_fits_u16`.
    #[allow(dead_code, reason = "exists only to provide a non-usize Pointee::Metadata type")]
    struct OddMetadataDst(str);

    // SAFETY: `OddMetadataDst` is never constructed, and no pointer to it is
    // ever formed or split via `ptr_meta`. The impl exists solely to give
    // `metadata_fits_u16` a metadata type (`u8`) whose size is neither 0 nor
    // `size_of::<usize>()`.
    unsafe impl Pointee for OddMetadataDst {
        type Metadata = u8;
    }

    /// Cover `metadata_fits_u16`'s non-`usize`-sized metadata reject branch:
    /// `size_of::<u8>()` is 1, which is neither 0 nor `size_of::<usize>()`.
    #[test]
    fn metadata_fits_u16_rejects_non_usize_metadata() {
        assert!(!metadata_fits_u16::<OddMetadataDst>(0u8));
    }
}
