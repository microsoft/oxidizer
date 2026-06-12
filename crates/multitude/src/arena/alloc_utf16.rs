// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! UTF-16 string allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself. Both `ArcUtf16Str` and
//! `BoxUtf16Str` share the same length-prefixed chunk layout
//! (`[usize u16-count][u16 elements]`, prefix unaligned) and are
//! thin 8-byte smart pointers.

use core::mem;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{AllocError, Allocator};

use super::alloc_prefixed::PREFIX_BYTES;
use super::alloc_value::acquire_shared_chunk_ref;
use super::{Arena, ExpectAlloc};

#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
impl<A: Allocator + Clone> Arena<A> {
    /// Copy `s` into a `Shared`-flavor chunk and return an [`ArcUtf16Str`](crate::strings::ArcUtf16Str).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_arc(&self, s: impl AsRef<widestring::Utf16Str>) -> crate::strings::ArcUtf16Str<A>
    where
        A: Send + Sync,
    {
        self.try_alloc_utf16_str_arc(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_utf16_str_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails (chunk
    /// exhaustion or oversize-cutover budget) or if the source string
    /// length would overflow the inline length-prefix accounting.
    #[inline]
    pub fn try_alloc_utf16_str_arc(&self, s: impl AsRef<widestring::Utf16Str>) -> Result<crate::strings::ArcUtf16Str<A>, AllocError>
    where
        A: Send + Sync,
    {
        self.impl_alloc_prefixed_shared::<u16>(s.as_ref().as_slice()).map(|ptr|
            // SAFETY: see `Self::alloc_utf16_str_arc`.
            unsafe { crate::strings::ArcUtf16Str::from_raw(ptr) })
    }

    /// Copy `s` into the arena and return a [`BoxUtf16Str`](crate::strings::BoxUtf16Str).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_box(&self, s: impl AsRef<widestring::Utf16Str>) -> crate::strings::BoxUtf16Str<A> {
        self.try_alloc_utf16_str_box(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_utf16_str_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails (chunk
    /// exhaustion or oversize-cutover budget) or if the source string
    /// length would overflow the inline length-prefix accounting.
    #[inline]
    pub fn try_alloc_utf16_str_box(&self, s: impl AsRef<widestring::Utf16Str>) -> Result<crate::strings::BoxUtf16Str<A>, AllocError> {
        self.impl_alloc_prefixed_shared::<u16>(s.as_ref().as_slice()).map(|ptr|
            // SAFETY: see `Self::alloc_utf16_str_arc`.
            unsafe { crate::strings::BoxUtf16Str::from_raw(ptr) })
    }

    /// Transcode `s` from UTF-8 to UTF-16 and return an [`ArcUtf16Str`](crate::strings::ArcUtf16Str).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_arc_from_str(&self, s: impl AsRef<str>) -> crate::strings::ArcUtf16Str<A>
    where
        A: Send + Sync,
    {
        self.try_alloc_utf16_str_arc_from_str(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_utf16_str_arc_from_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails (chunk
    /// exhaustion or oversize-cutover budget) or if the source string
    /// length would overflow the inline length-prefix accounting.
    #[inline]
    pub fn try_alloc_utf16_str_arc_from_str(&self, s: impl AsRef<str>) -> Result<crate::strings::ArcUtf16Str<A>, AllocError>
    where
        A: Send + Sync,
    {
        self.impl_alloc_utf16_prefixed_from_str(s.as_ref()).map(|ptr|
            // SAFETY: see `Self::alloc_utf16_str_arc`.
            unsafe { crate::strings::ArcUtf16Str::from_raw(ptr) })
    }

    /// Transcode `s` from UTF-8 to UTF-16 and return a [`BoxUtf16Str`](crate::strings::BoxUtf16Str).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_box_from_str(&self, s: impl AsRef<str>) -> crate::strings::BoxUtf16Str<A> {
        self.try_alloc_utf16_str_box_from_str(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_utf16_str_box_from_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails (chunk
    /// exhaustion or oversize-cutover budget) or if the source string
    /// length would overflow the inline length-prefix accounting.
    #[inline]
    pub fn try_alloc_utf16_str_box_from_str(&self, s: impl AsRef<str>) -> Result<crate::strings::BoxUtf16Str<A>, AllocError> {
        self.impl_alloc_utf16_prefixed_from_str(s.as_ref()).map(|ptr|
            // SAFETY: see `Self::alloc_utf16_str_arc`.
            unsafe { crate::strings::BoxUtf16Str::from_raw(ptr) })
    }

    /// Create a new, empty growable [`Utf16String`](crate::strings::Utf16String) backed by this arena.
    #[must_use]
    #[inline]
    pub const fn alloc_utf16_string(&self) -> crate::strings::Utf16String<'_, A> {
        crate::strings::Utf16String::new_in(self)
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
    pub fn alloc_utf16_string_with_capacity(&self, cap: usize) -> crate::strings::Utf16String<'_, A> {
        crate::strings::Utf16String::with_capacity_in(cap, self)
    }

    /// Fallible variant of [`Self::alloc_utf16_string_with_capacity`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails (chunk
    /// exhaustion or oversize-cutover budget) or if the source string
    /// length would overflow the inline length-prefix accounting.
    #[inline]
    pub fn try_alloc_utf16_string_with_capacity(&self, cap: usize) -> Result<crate::strings::Utf16String<'_, A>, AllocError> {
        crate::strings::Utf16String::try_with_capacity_in(cap, self)
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
        let exact: usize = s.chars().map(char::len_utf16).sum();
        let elem_size = mem::size_of::<u16>();
        let elem_align = mem::align_of::<u16>();
        // At least `elem_align` payload bytes so the returned pointer
        // is strictly inside the chunk (smart-pointer recovery
        // invariant).
        let payload_bytes = exact.checked_mul(elem_size).ok_or(AllocError)?.max(elem_align);
        let total = PREFIX_BYTES.checked_add(payload_bytes).ok_or(AllocError)?;
        loop {
            if let Some((uninit, chunk_ptr)) = self.current_shared().try_alloc_with_chunk(total, elem_align) {
                let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                let payload = transcode_utf16_into(uninit.as_non_null(), s, exact);
                let _ = chunk_ref.forget();
                #[cfg(feature = "stats")]
                self.record_alloc(exact * elem_size);
                return Ok(payload);
            }
            if self.is_oversized_shared(total) {
                return self.alloc_oversized_shared_with(total, |mutator, chunk_ptr| {
                    let (base, _chunk_unused) = mutator
                        .try_alloc_with_chunk(total, elem_align)
                        .expect("dedicated oversized chunk sized to fit utf-16 payload");
                    let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                    let payload = transcode_utf16_into(base.as_non_null(), s, exact);
                    let _ = chunk_ref.forget();
                    #[cfg(feature = "stats")]
                    self.record_alloc(exact * elem_size);
                    payload
                });
            }
            self.refill_shared(total)?;
        }
    }
}

/// Writes the `usize` element-count prefix at `base`, transcodes `s`
/// via `encode_utf16` into the `u16` payload immediately after, and
/// returns a thin pointer to the first payload element. `exact` must
/// match `s.chars().map(char::len_utf16).sum()` from a pre-walk.
//
// Skip `inline(always)` under coverage instrumentation so a real
// out-of-line function body exists for llvm-cov to attribute hits to.
// With `inline(always)` the source-line mapping for the inlined copies
// is fragile and shifts with the dep graph (observed: 11 lines went
// from 100% to missed when PR #478 added optional deps elsewhere).
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
