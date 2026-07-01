// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! UTF-16 string allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself. Both `Arc<Utf16Str>` and
//! `Box<Utf16Str>` share the same length-prefixed chunk layout
//! (`[usize u16-count][u16 elements]`, prefix unaligned) and are
//! thin 8-byte smart pointers.

use core::mem;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::Allocator;

use super::alloc_prefixed::PREFIX_BYTES;
use super::alloc_value::acquire_chunk_ref;
use super::{Arena, ExpectAlloc};
use crate::internal::thin_dst::{AtomicStrong, LocalStrong, Strong};
use crate::strings::{Utf16Str, Utf16String};
use crate::{AllocError, Arc, Box, Rc};

#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
impl<A: Allocator + Clone> Arena<A> {
    /// Copy `s` into a chunk and return an `Arc<Utf16Str>`.
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_arc(&self, s: impl AsRef<widestring::Utf16Str>) -> Arc<Utf16Str, A>
    where
        A: Send + Sync,
    {
        self.try_alloc_utf16_str_arc(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_utf16_str_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the source
    /// string is too large to allocate.
    #[inline]
    pub fn try_alloc_utf16_str_arc(&self, s: impl AsRef<widestring::Utf16Str>) -> Result<Arc<Utf16Str, A>, AllocError>
    where
        A: Send + Sync,
    {
        self.impl_alloc_utf16_str_smart::<AtomicStrong>(s.as_ref().as_slice())
    }

    /// Copy `s` into a chunk and return an `Rc<Utf16Str>` (non-atomic).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_rc(&self, s: impl AsRef<widestring::Utf16Str>) -> Rc<Utf16Str, A> {
        self.try_alloc_utf16_str_rc(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_utf16_str_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the source
    /// string is too large to allocate.
    #[inline]
    pub fn try_alloc_utf16_str_rc(&self, s: impl AsRef<widestring::Utf16Str>) -> Result<Rc<Utf16Str, A>, AllocError> {
        self.impl_alloc_utf16_str_smart::<LocalStrong>(s.as_ref().as_slice())
    }

    /// Copy a `u16` code-unit slice into a strong-prefixed chunk allocation and
    /// adopt it into `S`'s thin UTF-16 string smart pointer ([`Arc<Utf16Str>`]
    /// or [`Rc<Utf16Str>`]). The payload is laid out like a `[u16]` slice;
    /// `Utf16Str` reinterprets it (its metadata is the `u16` length).
    #[inline]
    fn impl_alloc_utf16_str_smart<S: Strong>(&self, units: &[u16]) -> Result<S::Ptr<Utf16Str, A>, AllocError> {
        let thin = self.impl_alloc_prefixed_shared_arc::<S, u16>(units)?;
        // SAFETY: `impl_alloc_prefixed_shared_arc` returns a thin pointer to a
        // `len`-prefixed `[u16]` payload whose chunk prefix holds a strong count
        // of 1 and whose chunk it took a `+1` on, all within the chunk's first
        // tile. `Utf16Str` and `[u16]` share that storage layout, so adopting it
        // as `S::Ptr<Utf16Str, A>` is sound — exactly `S::adopt`'s contract.
        Ok(unsafe { S::adopt::<Utf16Str, A>(thin.cast::<u8>()) })
    }

    /// Copy `s` into the arena and return a `Box<Utf16Str>`.
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_box(&self, s: impl AsRef<widestring::Utf16Str>) -> Box<Utf16Str, A> {
        self.try_alloc_utf16_str_box(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_utf16_str_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the source
    /// string is too large to allocate.
    #[inline]
    pub fn try_alloc_utf16_str_box(&self, s: impl AsRef<widestring::Utf16Str>) -> Result<Box<Utf16Str, A>, AllocError> {
        self.impl_alloc_prefixed_shared::<u16>(s.as_ref().as_slice()).map(|ptr|
            // SAFETY: `impl_alloc_prefixed_shared::<u16>` returns a thin payload
            // pointer to UTF-16 units copied into the arena, with the element
            // count written into the metadata prefix and a fresh chunk `+1`
            // adopted. `Utf16Str` and `[u16]` share that length-prefixed layout,
            // so `Box::from_raw` reconstructs an owning `Box<Utf16Str>`.
            unsafe { Box::<Utf16Str, A>::from_raw(ptr.cast::<u8>()) })
    }

    /// Transcode `s` from UTF-8 to UTF-16 and return an `Arc<Utf16Str>`.
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_arc_from_str(&self, s: impl AsRef<str>) -> Arc<Utf16Str, A>
    where
        A: Send + Sync,
    {
        self.try_alloc_utf16_str_arc_from_str(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_utf16_str_arc_from_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the source
    /// string is too large to allocate.
    #[inline]
    pub fn try_alloc_utf16_str_arc_from_str(&self, s: impl AsRef<str>) -> Result<Arc<Utf16Str, A>, AllocError>
    where
        A: Send + Sync,
    {
        self.impl_alloc_utf16_prefixed_from_str_arc::<AtomicStrong>(s.as_ref())
    }

    /// Transcode `s` from UTF-8 to UTF-16 and return an `Rc<Utf16Str>` (non-atomic).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_rc_from_str(&self, s: impl AsRef<str>) -> Rc<Utf16Str, A> {
        self.try_alloc_utf16_str_rc_from_str(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_utf16_str_rc_from_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the source
    /// string is too large to allocate.
    #[inline]
    pub fn try_alloc_utf16_str_rc_from_str(&self, s: impl AsRef<str>) -> Result<Rc<Utf16Str, A>, AllocError> {
        self.impl_alloc_utf16_prefixed_from_str_arc::<LocalStrong>(s.as_ref())
    }

    /// Transcode `s` from UTF-8 to UTF-16 and return a `Box<Utf16Str>`.
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_box_from_str(&self, s: impl AsRef<str>) -> Box<Utf16Str, A> {
        self.try_alloc_utf16_str_box_from_str(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_utf16_str_box_from_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the source
    /// string is too large to allocate.
    #[inline]
    pub fn try_alloc_utf16_str_box_from_str(&self, s: impl AsRef<str>) -> Result<Box<Utf16Str, A>, AllocError> {
        self.impl_alloc_utf16_prefixed_from_str(s.as_ref()).map(|ptr|
            // SAFETY: `impl_alloc_utf16_prefixed_from_str` transcodes `s` into
            // UTF-16 units in the arena, writes the element count into the
            // metadata prefix and adopts a fresh chunk `+1`. `Utf16Str` and
            // `[u16]` share that length-prefixed layout, so `Box::from_raw`
            // reconstructs an owning `Box<Utf16Str>`.
            unsafe { Box::<Utf16Str, A>::from_raw(ptr.cast::<u8>()) })
    }

    /// Create a new, empty growable [`Utf16String`](crate::strings::Utf16String) backed by this arena.
    #[must_use]
    #[inline]
    pub const fn alloc_utf16_string(&self) -> Utf16String<'_, A> {
        Utf16String::new_in(self)
    }

    /// Create a new growable arena-backed [`Utf16String`](crate::strings::Utf16String) with capacity.
    ///
    /// At least `cap` `u16` elements are pre-allocated.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Use
    /// [`Self::try_alloc_utf16_string_with_capacity`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_utf16_string_with_capacity(&self, cap: usize) -> Utf16String<'_, A> {
        Utf16String::with_capacity_in(cap, self)
    }

    /// Fallible variant of [`Self::alloc_utf16_string_with_capacity`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the source
    /// string is too large to allocate.
    #[inline]
    pub fn try_alloc_utf16_string_with_capacity(&self, cap: usize) -> Result<Utf16String<'_, A>, AllocError> {
        Utf16String::try_with_capacity_in(cap, self)
    }

    /// Shared body for `alloc_utf16_str_arc_from_str` /
    /// `alloc_utf16_str_box_from_str` (and their `try_*` siblings).
    ///
    /// Reserves a length-prefixed region (`[usize u16-count][u16 payload]`,
    /// payload `u16`-aligned, prefix unaligned) sized exactly to hold
    /// the UTF-16 transcoding of `s`, bumps the chunk's strong refcount
    /// by one for the new smart pointer, writes the prefix and
    /// in-place-transcoded `u16`s, and returns a thin `NonNull<u16>` to
    /// the first payload element.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // size-hint mutation ⇒ refill spin (OOM)
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "base reservation is align_of::<u16>() (passed to try_alloc); `base + PREFIX_BYTES` is u16-aligned"
    )]
    fn impl_alloc_utf16_prefixed_from_str(&self, s: &str) -> Result<NonNull<u16>, AllocError> {
        // `encode_utf16` is lazy; pre-walking is O(n) but lets us size
        // the reservation exactly without over-allocating for ASCII.
        // Each char's UTF-16 unit count never exceeds its UTF-8 byte length, so
        // this total can never exceed `s.len()` (which fits in `isize`); the
        // checked accumulation makes that no-overflow guarantee explicit.
        let exact = s
            .chars()
            .try_fold(0_usize, |acc, c| acc.checked_add(c.len_utf16()))
            .ok_or(AllocError::CAPACITY_OVERFLOW)?;
        let elem_size = mem::size_of::<u16>();
        let elem_align = mem::align_of::<u16>();
        // At least `elem_align` payload bytes so the returned pointer
        // is strictly inside the chunk (smart-pointer recovery
        // invariant).
        let payload_bytes = exact.checked_mul(elem_size).ok_or(AllocError::CAPACITY_OVERFLOW)?.max(elem_align);
        let total = PREFIX_BYTES.checked_add(payload_bytes).ok_or(AllocError::CAPACITY_OVERFLOW)?;
        loop {
            if let Some((uninit, chunk_ptr)) = self.current().try_alloc_with_chunk(total, elem_align) {
                let chunk_ref = self.acquire_current_chunk_ref(chunk_ptr);
                let payload = transcode_utf16_into(uninit.as_non_null(), s, exact);
                let _ = chunk_ref.forget();
                return Ok(payload);
            }
            if self.is_oversized(total) {
                return self.alloc_oversized_shared_with(total, |mutator, chunk_ptr| {
                    let (base, _chunk_unused) = mutator
                        .try_alloc_with_chunk(total, elem_align)
                        .expect("dedicated oversized chunk sized to fit utf-16 payload");
                    let chunk_ref = acquire_chunk_ref::<A>(chunk_ptr);
                    let payload = transcode_utf16_into(base.as_non_null(), s, exact);
                    let _ = chunk_ref.forget();
                    payload
                });
            }
            self.refill(total)?;
        }
    }

    /// Strong-prefixed `Arc<Utf16Str>`
    /// variant of [`Self::impl_alloc_utf16_prefixed_from_str`]: reserves
    /// a per-`Arc` strong count and slice-length prefix, transcodes `s`
    /// into the `u16` payload, and returns a thin pointer to the first
    /// payload element.
    /// Transcode `s` from UTF-8 to UTF-16 into a strong-prefixed chunk
    /// allocation and adopt it into `S`'s thin UTF-16 string smart pointer.
    #[inline(always)]
    fn impl_alloc_utf16_prefixed_from_str_arc<S: Strong>(&self, s: &str) -> Result<S::Ptr<Utf16Str, A>, AllocError> {
        let thin = self.alloc_utf16_prefixed_from_str_raw::<S>(s)?;
        // SAFETY: `alloc_utf16_prefixed_from_str_raw` returns a thin pointer to
        // a `len`-prefixed `[u16]` payload whose chunk prefix holds a strong
        // count of 1 and whose chunk it took a `+1` on, all within the chunk's
        // first tile. `Utf16Str` shares the `[u16]` storage layout, so adopting
        // it as `S::Ptr<Utf16Str, A>` is sound — exactly `S::adopt`'s contract.
        Ok(unsafe { S::adopt::<Utf16Str, A>(thin.cast::<u8>()) })
    }

    /// Raw UTF-8→UTF-16 transcode returning the thin `u16` payload pointer
    /// (before adoption). Split out so the single `S::adopt` lives in
    /// [`Self::impl_alloc_utf16_prefixed_from_str_arc`].
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // size-hint mutation ⇒ refill spin (OOM)
    fn alloc_utf16_prefixed_from_str_raw<S: Strong>(&self, s: &str) -> Result<NonNull<u16>, AllocError> {
        // Each char's UTF-16 unit count never exceeds its UTF-8 byte length, so
        // this total can never exceed `s.len()` (which fits in `isize`); the
        // checked accumulation makes that no-overflow guarantee explicit and
        // returns `AllocError` rather than wrapping should the invariant ever be
        // violated.
        let exact = s
            .chars()
            .try_fold(0_usize, |acc, c| acc.checked_add(c.len_utf16()))
            .ok_or(AllocError::CAPACITY_OVERFLOW)?;
        let bytes_needed = super::alloc_prefixed::worst_case_strong_slice_payload::<S, u16>(exact);
        loop {
            if let Some((uninit, chunk_ptr)) = self.try_reserve_arc_slice::<S, u16>(exact) {
                let chunk_ref = self.acquire_current_chunk_ref(chunk_ptr);
                let payload = uninit.init_from_iter_ptr(s.encode_utf16());
                let _ = chunk_ref.forget();
                return Ok(payload.cast::<u16>());
            }
            if self.is_oversized(bytes_needed) {
                return self.alloc_oversized_shared_with(bytes_needed, |mutator, chunk_ptr| {
                    let (ticket, _chunk) = mutator
                        .try_alloc_arc_slice::<S, u16>(exact)
                        .expect("dedicated oversized chunk sized to fit utf-16 Arc payload");
                    let chunk_ref = acquire_chunk_ref::<A>(chunk_ptr);
                    let payload = ticket.init_from_iter_ptr(s.encode_utf16());
                    let _ = chunk_ref.forget();
                    payload.cast::<u16>()
                });
            }
            self.refill(bytes_needed)?;
        }
    }
}

/// Writes the `usize` element-count prefix at `base`, transcodes `s`
/// via `encode_utf16` into the `u16` payload immediately after, and
/// returns a thin pointer to the first payload element. `exact` must
/// match `s.chars().map(char::len_utf16).sum()` from a pre-walk.
//
// Skip `inline(always)` under coverage: a real out-of-line body lets
// llvm-cov attribute hits reliably, since the source-line mapping for
// inlined copies is fragile and shifts with the dep graph.
#[cfg_attr(not(coverage_nightly), inline(always))]
#[allow(
    clippy::cast_ptr_alignment,
    reason = "see callers: `base + PREFIX_BYTES` is u16-aligned by construction"
)]
fn transcode_utf16_into(base: NonNull<u8>, s: &str, exact: usize) -> NonNull<u16> {
    // SAFETY: caller's reservation owns `PREFIX_BYTES + exact * 2` bytes
    // at `base`, `base + PREFIX_BYTES` is u16-aligned, the transcode
    // produces exactly `exact` units (matched by the pre-walk), and no
    // other reference points at this region.
    unsafe {
        ptr::write_unaligned(base.as_ptr().cast::<usize>(), exact);
        let payload_ptr = base.as_ptr().add(PREFIX_BYTES).cast::<u16>();
        let mut written = 0_usize;
        for u in s.encode_utf16() {
            debug_assert!(written < exact, "transcoded count exceeded pre-computed length");
            payload_ptr.add(written).write(u);
            written += 1;
        }
        debug_assert_eq!(written, exact, "transcoded count differed from pre-computed length");
        NonNull::new_unchecked(payload_ptr)
    }
}
