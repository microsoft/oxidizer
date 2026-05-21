// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! UTF-16 string allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.

use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::{AllocFlavor, Arena, expect_alloc};
use crate::internal::owned_in_chunk::{OwnedInLocalChunk, OwnedInSharedChunk};

#[cfg_attr(docsrs, doc(cfg(feature = "utf16")))]
impl<A: Allocator + Clone> Arena<A> {
    /// Allocates `[len_prefix: usize][u16 elements...]` in the current
    /// local chunk, copies elements, and bumps refcount per `flavor`.
    fn try_alloc_utf16_prefixed_local(&self, src: &[u16], flavor: AllocFlavor) -> Result<NonNull<u16>, AllocError> {
        let len = src.len();
        let payload_bytes = len.checked_mul(core::mem::size_of::<u16>()).ok_or(AllocError)?;
        let total = core::mem::size_of::<usize>().checked_add(payload_bytes).ok_or(AllocError)?;
        if total > self.provider.max_normal_alloc {
            debug_assert!(matches!(flavor, AllocFlavor::Rc | AllocFlavor::Box));
            // SAFETY: src is a valid slice of `len` u16s.
            return unsafe { self.try_alloc_prefixed_local_oversized::<u16>(src.as_ptr(), len, payload_bytes) };
        }
        try_alloc_prefixed!(
            self = self,
            src_ptr = src.as_ptr(),
            len = len,
            payload_bytes = payload_bytes,
            elem_ty = u16,
            slot = current_local,
            refill = refill_local,
            accounting = {
                // SAFETY: callers only invoke with `Rc` or `Box`; the `SimpleRef`
                // flavor goes through a different code path.
                debug_assert!(matches!(flavor, AllocFlavor::Rc | AllocFlavor::Box));
                self.current_local.bump_smart_pointers_issued();
            },
        )
    }

    /// Local owned sibling for the safe smart-pointer constructors.
    fn try_alloc_utf16_prefixed_local_owned(&self, src: &[u16], flavor: AllocFlavor) -> Result<OwnedInLocalChunk<u16, A>, AllocError> {
        let raw = self.try_alloc_utf16_prefixed_local(src, flavor)?;
        // SAFETY: helper returned a fresh local UTF-16 allocation with one caller-owned `+1`.
        Ok(unsafe { OwnedInLocalChunk::from_raw_alloc(raw) })
    }

    /// Allocates `[len_prefix: usize][u16 elements...]` in the current
    /// shared chunk, accounting via `arcs_issued`.
    fn try_alloc_utf16_prefixed_shared(&self, src: &[u16]) -> Result<NonNull<u16>, AllocError> {
        let len = src.len();
        let payload_bytes = len.checked_mul(core::mem::size_of::<u16>()).ok_or(AllocError)?;
        let total = core::mem::size_of::<usize>().checked_add(payload_bytes).ok_or(AllocError)?;
        if total > self.provider.max_normal_alloc {
            // SAFETY: src is a valid slice of `len` u16s.
            return unsafe { self.try_alloc_prefixed_shared_oversized::<u16>(src.as_ptr(), len, payload_bytes) };
        }
        try_alloc_prefixed!(
            self = self,
            src_ptr = src.as_ptr(),
            len = len,
            payload_bytes = payload_bytes,
            elem_ty = u16,
            slot = current_shared,
            refill = refill_shared,
            accounting = {
                self.current_shared.bump_smart_pointers_issued();
            },
        )
    }

    /// Shared owned sibling of [`Self::try_alloc_utf16_prefixed_local_owned`].
    fn try_alloc_utf16_prefixed_shared_owned(&self, src: &[u16]) -> Result<OwnedInSharedChunk<u16, A>, AllocError> {
        let raw = self.try_alloc_utf16_prefixed_shared(src)?;
        // SAFETY: helper returned a fresh shared UTF-16 allocation with one caller-owned `+1`.
        Ok(unsafe { OwnedInSharedChunk::from_raw_alloc(raw) })
    }

    /// Copy `s` into the arena and return an [`RcUtf16Str`](crate::strings::RcUtf16Str).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_rc(&self, s: impl AsRef<widestring::Utf16Str>) -> crate::strings::RcUtf16Str<A> {
        expect_alloc(self.try_alloc_utf16_str_rc(s))
    }

    /// Fallible variant of [`Self::alloc_utf16_str_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails (chunk
    /// exhaustion or oversize-cutover budget) or if the source string
    /// length would overflow the inline length-prefix accounting.
    #[inline]
    pub fn try_alloc_utf16_str_rc(&self, s: impl AsRef<widestring::Utf16Str>) -> Result<crate::strings::RcUtf16Str<A>, AllocError> {
        let owned = self.try_alloc_utf16_prefixed_local_owned(s.as_ref().as_slice(), AllocFlavor::Rc)?;
        Ok(crate::strings::RcUtf16Str::from_owned_in_chunk(owned))
    }

    /// Copy `s` into a `Shared`-flavor chunk and return an [`ArcUtf16Str`](crate::strings::ArcUtf16Str).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_arc(&self, s: impl AsRef<widestring::Utf16Str>) -> crate::strings::ArcUtf16Str<A>
    where
        A: Send + Sync,
    {
        expect_alloc(self.try_alloc_utf16_str_arc(s))
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
        let owned = self.try_alloc_utf16_prefixed_shared_owned(s.as_ref().as_slice())?;
        Ok(crate::strings::ArcUtf16Str::from_owned_in_chunk(owned))
    }

    /// Copy `s` into the arena and return a [`BoxUtf16Str`](crate::strings::BoxUtf16Str).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_box(&self, s: impl AsRef<widestring::Utf16Str>) -> crate::strings::BoxUtf16Str<A> {
        expect_alloc(self.try_alloc_utf16_str_box(s))
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
        let owned = self.try_alloc_utf16_prefixed_local_owned(s.as_ref().as_slice(), AllocFlavor::Box)?;
        Ok(crate::strings::BoxUtf16Str::from_owned_in_chunk(owned))
    }

    /// Transcode `s` from UTF-8 to UTF-16 and return an [`RcUtf16Str`](crate::strings::RcUtf16Str).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_rc_from_str(&self, s: impl AsRef<str>) -> crate::strings::RcUtf16Str<A> {
        expect_alloc(self.try_alloc_utf16_str_rc_from_str(s))
    }

    /// Fallible variant of [`Self::alloc_utf16_str_rc_from_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails (chunk
    /// exhaustion or oversize-cutover budget) or if the source string
    /// length would overflow the inline length-prefix accounting.
    #[inline]
    pub fn try_alloc_utf16_str_rc_from_str(&self, s: impl AsRef<str>) -> Result<crate::strings::RcUtf16Str<A>, AllocError> {
        let buf: alloc::vec::Vec<u16> = s.as_ref().encode_utf16().collect();
        let owned = self.try_alloc_utf16_prefixed_local_owned(&buf, AllocFlavor::Rc)?;
        Ok(crate::strings::RcUtf16Str::from_owned_in_chunk(owned))
    }

    /// Transcode `s` from UTF-8 to UTF-16 and return an [`ArcUtf16Str`](crate::strings::ArcUtf16Str).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_arc_from_str(&self, s: impl AsRef<str>) -> crate::strings::ArcUtf16Str<A>
    where
        A: Send + Sync,
    {
        expect_alloc(self.try_alloc_utf16_str_arc_from_str(s))
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
        let buf: alloc::vec::Vec<u16> = s.as_ref().encode_utf16().collect();
        let owned = self.try_alloc_utf16_prefixed_shared_owned(&buf)?;
        Ok(crate::strings::ArcUtf16Str::from_owned_in_chunk(owned))
    }

    /// Transcode `s` from UTF-8 to UTF-16 and return a [`BoxUtf16Str`](crate::strings::BoxUtf16Str).
    #[must_use]
    #[inline]
    pub fn alloc_utf16_str_box_from_str(&self, s: impl AsRef<str>) -> crate::strings::BoxUtf16Str<A> {
        expect_alloc(self.try_alloc_utf16_str_box_from_str(s))
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
        let buf: alloc::vec::Vec<u16> = s.as_ref().encode_utf16().collect();
        let owned = self.try_alloc_utf16_prefixed_local_owned(&buf, AllocFlavor::Box)?;
        Ok(crate::strings::BoxUtf16Str::from_owned_in_chunk(owned))
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
}
