// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Length-prefixed chunk allocator for thin string smart pointers
//! ([`Arc<str>`](crate::Arc), [`Box<str>`](crate::Box),
//! `Arc<Utf16Str>`, `Box<Utf16Str>`) and the generic thin
//! DST `Arc<[T]>` / `Box<[T]>` family.
//!
//! Layout: `[usize len (unaligned)][len * T payload]`. The payload is
//! T-aligned because the reservation itself is T-aligned and
//! `PREFIX_BYTES` is a multiple of any supported T's align. The leading
//! `usize` is read unaligned, so consecutive prefixed allocations pack
//! without per-allocation alignment padding for the length field.
//!
//! Restriction: `align_of::<T>() <= align_of::<usize>()` (statically
//! asserted). All current callers use `u8`, `u16`, or `usize`.

use core::mem;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{AllocError, Allocator};

use super::Arena;
use super::alloc_value::acquire_chunk_ref;
use crate::internal::chunk_ref::ChunkRef;
use crate::internal::drop_entry::DropEntry;

/// Byte size of the inline element-count prefix written immediately
/// before every prefixed-shared payload.
pub(in crate::arena) const PREFIX_BYTES: usize = mem::size_of::<usize>();

/// Worst-case byte budget for a thin-pointer smart-slice allocation of
/// `len` elements of type `T` (used by [`Arc<[T]>`](crate::Arc) /
/// [`Box<[T]>`](crate::Box) refill hints).
///
/// Includes the length prefix + payload alignment slack + payload
/// bytes + (if `T: Drop`) one drop-entry slot. Saturates at
/// `usize::MAX` on overflow.
#[inline]
#[cfg_attr(test, mutants::skip)] // underestimating refill hint ⇒ refill spin
pub(in crate::arena) fn worst_case_thin_slice_payload<T>(len: usize) -> usize {
    let elem_size = mem::size_of::<T>();
    let elem_align = mem::align_of::<T>();
    let payload_offset = PREFIX_BYTES.max(elem_align);
    let value_bytes = elem_size.saturating_mul(len);
    let base = payload_offset
        .saturating_add(value_bytes)
        // Account for try_alloc's possible alignment padding (one
        // worst-case align-up at the front of the reservation).
        .saturating_add(elem_align);
    if mem::needs_drop::<T>() {
        base.saturating_add(mem::size_of::<DropEntry>())
            .saturating_add(mem::align_of::<DropEntry>())
    } else {
        base
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Reserves `PREFIX_BYTES + max(src.len() * size_of::<T>(),
    /// align_of::<T>())` bytes in the current chunk, writes the
    /// length prefix (unaligned) and the payload, bumps the chunk's
    /// strong refcount by one for the new smart pointer, and returns a
    /// thin `NonNull<T>` to the first payload element.
    ///
    /// `T` must have `align_of::<T>() <= align_of::<usize>()`; see
    /// module docs.
    #[inline(always)]
    pub(in crate::arena) fn impl_alloc_prefixed_shared<T: Copy>(&self, src: &[T]) -> Result<NonNull<T>, AllocError> {
        const {
            assert!(
                mem::align_of::<T>() <= mem::align_of::<usize>(),
                "impl_alloc_prefixed_shared: T's align must not exceed usize's align (PREFIX_BYTES would otherwise not guarantee payload alignment)",
            );
        }
        let elem_size = mem::size_of::<T>();
        let elem_align = mem::align_of::<T>();
        let len = src.len();
        // Payload is at least `elem_align` bytes so the returned payload
        // pointer is strictly inside the chunk (never one-past-end at
        // `chunk_base + CHUNK_ALIGN`), preserving the mask-based chunk
        // recovery invariant used by the smart pointers' `Drop`.
        let payload_bytes = len.checked_mul(elem_size).ok_or(AllocError)?.max(elem_align);
        let total = PREFIX_BYTES.checked_add(payload_bytes).ok_or(AllocError)?;
        // `total` is an exact reservation size, not a worst-case hint: unlike
        // the slice paths (which permit over-aligned `T` and so add `elem_align`
        // of front-padding slack to their routing hint), the const-assert above
        // bounds `elem_align <= align_of::<usize>() <= value_align`. A fresh
        // chunk's payload base is `value_align`-aligned, so an `elem_align`
        // reservation on a freshly refilled chunk never consumes front padding.
        // Routing/refilling with `total` therefore always yields a chunk into
        // which the retry's `try_alloc_with_chunk(total, elem_align)` fits — no
        // `total` vs `total + elem_align` boundary loop is possible here.
        loop {
            // Allocate `total` bytes aligned to `align_of::<T>()` so the
            // payload (at offset PREFIX_BYTES, a multiple of any align
            // ≤ usize's) ends up naturally aligned for `T` reads/writes.
            if let Some((uninit, chunk_ptr)) = self.current().try_alloc_with_chunk(total, elem_align) {
                let chunk_ref: ChunkRef<A> = self.acquire_current_chunk_ref(chunk_ptr);
                let payload = write_prefixed_payload::<T>(uninit.as_non_null(), src);
                // Hand the +1 over to the caller's smart pointer.
                let _ = chunk_ref.forget();
                return Ok(payload);
            }
            if self.is_oversized(total) {
                return self.alloc_oversized_shared_with(total, |mutator, chunk_ptr| {
                    let (base, _chunk_unused) = mutator
                        .try_alloc_with_chunk(total, elem_align)
                        .expect("dedicated oversized chunk sized to fit prefixed payload");
                    let chunk_ref: ChunkRef<A> = acquire_chunk_ref::<A>(chunk_ptr);
                    let payload = write_prefixed_payload::<T>(base.as_non_null(), src);
                    let _ = chunk_ref.forget();
                    payload
                });
            }
            self.refill(total)?;
        }
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Strong-prefixed [`Arc`](crate::Arc) variant of
    /// [`Self::impl_alloc_prefixed_shared`]: reserves a per-`Arc`
    /// [`AtomicU32`](core::sync::atomic::AtomicU32) strong count and a
    /// `usize` length metadata word immediately before the payload,
    /// initializes the strong count to `1`, writes the length and the
    /// payload, takes one chunk refcount for the new `Arc` family, and
    /// returns a thin `NonNull<T>` to the first payload element.
    ///
    /// `T` must have `align_of::<T>() <= align_of::<usize>()`; see
    /// module docs.
    #[inline(always)]
    pub(in crate::arena) fn impl_alloc_prefixed_shared_arc<T: Copy>(&self, src: &[T]) -> Result<NonNull<T>, AllocError> {
        const {
            assert!(
                mem::align_of::<T>() <= mem::align_of::<usize>(),
                "impl_alloc_prefixed_shared_arc: T's align must not exceed usize's align",
            );
        }
        let len = src.len();
        // `src` is a live `&[T]`, so `size_of_val(src)` is a valid usize.
        let payload_bytes = mem::size_of_val(src);
        let bytes_needed = worst_case_arc_slice_payload::<T>(len);
        loop {
            // SAFETY: `payload_bytes == size_of_val(src) == size_of::<T>() * len`.
            let reserved = unsafe { self.try_reserve_arc_slice_with_size::<T>(len, payload_bytes) };
            if let Some((uninit, chunk_ptr)) = reserved {
                let chunk_ref: ChunkRef<A> = self.acquire_current_chunk_ref(chunk_ptr);
                let slice_ptr = uninit.init_copy_from_slice_ptr(src);
                let _ = chunk_ref.forget();
                return Ok(slice_ptr.cast::<T>());
            }
            if self.is_oversized(bytes_needed) {
                return self.alloc_oversized_shared_with(bytes_needed, |mutator, chunk_ptr| {
                    let (ticket, _chunk) = mutator
                        .try_alloc_arc_slice::<T>(len)
                        .expect("dedicated oversized chunk sized to fit prefixed Arc payload");
                    let chunk_ref: ChunkRef<A> = acquire_chunk_ref::<A>(chunk_ptr);
                    let slice_ptr = ticket.init_copy_from_slice_ptr(src);
                    let _ = chunk_ref.forget();
                    slice_ptr.cast::<T>()
                });
            }
            self.refill(bytes_needed)?;
        }
    }
}

/// Worst-case byte budget for a strong-prefixed `Arc` slice/prefixed
/// payload of `len` elements: per-`Arc` strong count + slice-length
/// prefix + payload + front alignment slack. Shared by the `Arc<[T]>`,
/// `Arc<str>`, and `Arc<Utf16Str>` allocation paths.
#[cfg_attr(test, mutants::skip)] // underestimating refill hint ⇒ refill spin
#[inline]
pub(crate) fn worst_case_arc_slice_payload<T>(len: usize) -> usize {
    use crate::internal::thin_dst;
    let align = mem::align_of::<T>();
    let value_bytes = mem::size_of::<T>().saturating_mul(len).max(1);
    thin_dst::strong_prefix_bytes_for(align, mem::size_of::<usize>())
        .saturating_add(value_bytes)
        .saturating_add(thin_dst::arc_block_align(align))
}

/// Write the length prefix (unaligned `usize`) at `base` and copy
/// `src` immediately after, returning a thin pointer to the first
/// payload element.
///
/// Shared between the in-arena fast path and the dedicated-oversized
/// path in [`Arena::impl_alloc_prefixed_shared`]; isolating the
/// unsafe write to one place keeps the call sites trivial.
#[inline(always)]
fn write_prefixed_payload<T: Copy>(base: NonNull<u8>, src: &[T]) -> NonNull<T> {
    let len = src.len();
    // SAFETY: `base` references at least `PREFIX_BYTES + len * size_of::<T>()`
    // bytes of exclusively-owned chunk storage aligned to `align_of::<T>()`
    // (caller's reservation). `write_unaligned::<usize>` needs only u8
    // alignment. `base + PREFIX_BYTES` is T-aligned because PREFIX_BYTES
    // is a multiple of any align ≤ usize's. The payload slot covers
    // `len * elem_size` bytes (the empty-`src` floor in the caller only
    // matters when no copy happens).
    unsafe {
        ptr::write_unaligned(base.as_ptr().cast::<usize>(), len);
        let payload_ptr = base.as_ptr().add(PREFIX_BYTES).cast::<T>();
        ptr::copy_nonoverlapping(src.as_ptr(), payload_ptr, len);
        NonNull::new_unchecked(payload_ptr)
    }
}
