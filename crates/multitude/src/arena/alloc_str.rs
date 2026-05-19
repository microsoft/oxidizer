// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `&str` allocation API on [`Arena`].
//!
//! Public methods (`alloc_str`, `alloc_str_rc`/`arc`/`box`, plus `try_*`
//! variants) and their private helpers `try_alloc_str_inner` /
//! `alloc_str_inner_or_panic` / `try_alloc_str_prefixed_local` /
//! `try_alloc_str_prefixed_shared` are grouped here to keep the
//! central `mod.rs` smaller.

use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::{AllocFlavor, Arena, expect_alloc};
use crate::internal::local_chunk::LocalChunk;
use crate::internal::owned_in_chunk::{OwnedInLocalChunk, OwnedInSharedChunk};
use crate::strings::{ArcStr, BoxStr, RcStr};

impl<A: Allocator + Clone> Arena<A> {
    /// Bump-allocate a copy of `s` and return a mutable string slice.
    ///
    /// The returned `&mut str`'s lifetime is tied to `&self`. Like
    /// [`Self::alloc`] but for `&str`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails.
    /// Use [`Self::try_alloc_str`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let s: &mut str = arena.alloc_str("hello");
    /// s.make_ascii_uppercase();
    /// assert_eq!(s, "HELLO");
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_str(&self, s: impl AsRef<str>) -> &mut str {
        self.alloc_str_inner_or_panic(s.as_ref())
    }

    /// Copy `s` into the arena and return an [`RcStr`](crate::strings::RcStr) smart pointer.
    ///
    /// `RcStr` is a single-pointer, refcounted, `!Send`/`!Sync` immutable
    /// string.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails.
    /// Use [`Self::try_alloc_str_rc`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let s = arena.alloc_str_rc("hello");
    /// assert_eq!(&*s, "hello");
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_str_rc(&self, s: impl AsRef<str>) -> RcStr<A> {
        expect_alloc(self.try_alloc_str_rc(s))
    }

    /// Copy `s` into the arena and return an [`ArcStr`](crate::strings::ArcStr)
    /// pointing to it.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails.
    /// Use [`Self::try_alloc_str_arc`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let s = arena.alloc_str_arc("hello");
    /// assert_eq!(&*s, "hello");
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_str_arc(&self, s: impl AsRef<str>) -> ArcStr<A>
    where
        A: Send + Sync,
    {
        expect_alloc(self.try_alloc_str_arc(s))
    }

    /// Copy `s` into the arena and return a [`BoxStr`](crate::strings::BoxStr) smart pointer.
    ///
    /// `BoxStr` is a single-pointer (8 bytes) owned, mutable string.
    ///
    /// Compared to [`Self::alloc_str_rc`] / [`Self::alloc_str_arc`]:
    /// the box is `!Clone` (single-owner) but supports `&mut str` access
    /// and releases its chunk hold the moment it is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails.
    /// Use [`Self::try_alloc_str_box`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut s = arena.alloc_str_box("hello");
    /// s.make_ascii_uppercase();
    /// assert_eq!(&*s, "HELLO");
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_str_box(&self, s: impl AsRef<str>) -> BoxStr<A> {
        expect_alloc(self.try_alloc_str_box(s))
    }

    /// Fallible variant of [`Self::alloc_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    #[inline]
    pub fn try_alloc_str(&self, s: impl AsRef<str>) -> Result<&mut str, AllocError> {
        self.try_alloc_str_inner(s.as_ref())
    }

    /// Specialized fast path for `&str` allocation.
    ///
    /// Bypasses the generic [`Self::try_alloc_slice_copy`] and inlines a
    /// bump fit-check that subsumes the `worst_case_size` and align
    /// guards, plus a one-time `set_pinned(true)` after the first
    /// allocation in each chunk.
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "hot path; force-inlining lets LLVM see the &str fat-pointer through AsRef and skip stack materialization"
    )]
    fn try_alloc_str_inner(&self, s: &str) -> Result<&mut str, AllocError> {
        self.impl_alloc_str_inner::<false>(s)
    }

    /// Single source of truth for the `&str` fast path. `PANIC=true`
    /// panics on chunk-allocation failure; `PANIC=false` propagates
    /// `Err`.
    #[expect(clippy::mut_from_ref, reason = "simple references: see Self::try_alloc_with")]
    #[expect(
        clippy::inline_always,
        reason = "hot path; force-inlining lets LLVM see the &str fat-pointer through AsRef and skip stack materialization; PANIC const must fold"
    )]
    #[inline(always)]
    fn impl_alloc_str_inner<const PANIC: bool>(&self, s: &str) -> Result<&mut str, AllocError> {
        let bytes = s.as_bytes();
        let len = bytes.len();
        let data_ptr = self.current_local.data_ptr.get();
        let drop_back_ptr = self.current_local.drop_back.get();
        let drop_back_addr = drop_back_ptr.as_ptr() as usize;
        let data_ptr_addr = data_ptr.as_ptr() as usize;
        // SAFETY: `s` is a `&str`; the slice invariant guarantees
        // `s.len() <= isize::MAX`. `data_ptr` either points into a
        // live chunk payload (whose top sits well below `isize::MAX`
        // on every real platform) or is the stub `NonNull::dangling()`
        // at address 1 — both `<= isize::MAX as usize`. With both
        // bounds asserted, `data_ptr_addr + len <= 2 * isize::MAX <
        // usize::MAX`, so the sum cannot overflow and the
        // `checked_add` collapses to a plain `add` after inlining.
        unsafe {
            core::hint::assert_unchecked(isize::try_from(len).is_ok());
            core::hint::assert_unchecked(isize::try_from(data_ptr_addr).is_ok());
        }
        let end_addr = data_ptr_addr + len;
        if end_addr <= drop_back_addr {
            // `len == 0` in stub state: end_addr == data_ptr_addr == drop_back_addr,
            // so the bump check passes; we copy 0 bytes and the dangling
            // pointer is never dereferenced. data_ptr stays unchanged.
            let dest = data_ptr.as_ptr();
            // Publish the new bump cursor BEFORE the memcpy so the next
            // iteration's `data_ptr.get()` load can satisfy via
            // store-forwarding without waiting for the copy's stores to
            // drain (see `try_alloc_slice_local_copy` for the rationale).
            // SAFETY: `end_addr <= drop_back`, so `data_ptr + len` is in payload.
            let end_ptr = unsafe { data_ptr.byte_add(len) };
            self.current_local.data_ptr.set(end_ptr);
            self.current_local_pinned.set(true);
            // SAFETY: source and destination are valid for `len` bytes and non-overlapping.
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, len) };
            self.charge_alloc_stats(len);
            // SAFETY: bytes came from a valid `str` and were just copied into uniquely-owned storage.
            let slice = unsafe { core::slice::from_raw_parts_mut(dest, len) };
            // SAFETY: `slice` contains the copied UTF-8 bytes of `s`.
            return Ok(unsafe { core::str::from_utf8_unchecked_mut(slice) });
        }
        // Bump miss: route oversized requests directly to the one-shot
        // oversized helper. `refill_local` (used by the normal-size
        // slow path) rejects `len > MAX_CHUNK_BYTES` up front, so we
        // must handle that case before falling into it. There is no
        // intrinsic size limit on `alloc_str`.
        let r = if len > self.provider.max_normal_alloc {
            self.alloc_str_oversized(bytes)
        } else {
            self.alloc_str_inner_slow(bytes)
        };
        if PANIC { Ok(expect_alloc(r)) } else { r }
    }

    /// Cold refill-and-retry path for [`Self::impl_alloc_str_inner`].
    /// Kept out of line so the fast path stays small.
    #[cold]
    #[inline(never)]
    #[expect(clippy::mut_from_ref, reason = "simple references: see Self::try_alloc_with")]
    // EQUIVALENCE: `end_addr` is only used by the `debug_assert!` below.
    // The real cursor advance uses `byte_add(len)`, so the `+ -> -`
    // mutant changes no observable behavior.
    #[cfg_attr(test, mutants::skip)]
    fn alloc_str_inner_slow(&self, bytes: &[u8]) -> Result<&mut str, AllocError> {
        let len = bytes.len();
        // SAFETY: re-establish the bound asserted by the caller; `&[u8]` has
        // `len <= isize::MAX`.
        unsafe { core::hint::assert_unchecked(isize::try_from(len).is_ok()) };
        self.refill_local(len)?;
        let data_ptr = self.current_local.data_ptr.get();
        let drop_back_ptr = self.current_local.drop_back.get();
        let drop_back_addr = drop_back_ptr.as_ptr() as usize;
        let data_ptr_addr = data_ptr.as_ptr() as usize;
        // SAFETY: same bounds as the fast path.
        unsafe { core::hint::assert_unchecked(isize::try_from(data_ptr_addr).is_ok()) };
        let end_addr = data_ptr_addr + len;
        // `refill_local(len)` gives us at least `len` bytes, and `u8`
        // needs no extra alignment.
        debug_assert!(end_addr <= drop_back_addr, "refill_local guarantees a fitting chunk for alloc_str");
        let dest = data_ptr.as_ptr();
        // SAFETY: fit gate above.
        let end_ptr = unsafe { data_ptr.byte_add(len) };
        self.current_local.data_ptr.set(end_ptr);
        self.current_local_pinned.set(true);
        // SAFETY: source and destination are valid for `len` bytes and non-overlapping.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, len) };
        self.charge_alloc_stats(len);
        // SAFETY: bytes came from a valid `str` and were just copied.
        let slice = unsafe { core::slice::from_raw_parts_mut(dest, len) };
        // SAFETY: the caller produced `bytes` from a valid `str`.
        Ok(unsafe { core::str::from_utf8_unchecked_mut(slice) })
    }

    /// Cold oversized path for [`Self::alloc_str`] / [`Self::try_alloc_str`].
    ///
    /// It copies into a dedicated local chunk, pins that chunk for the
    /// returned `&mut str`, and reconciles `LARGE` down to the pin's `+1`.
    #[cold]
    #[inline(never)]
    #[expect(clippy::mut_from_ref, reason = "simple references: see Self::try_alloc_with")]
    fn alloc_str_oversized(&self, bytes: &[u8]) -> Result<&mut str, AllocError> {
        let len = bytes.len();
        let chunk = self.provider.acquire_local(len.max(1))?;
        // SAFETY: refcount-positive — chunk held at LARGE inflation
        // immediately after `acquire_local`.
        let chunk_ref = unsafe { chunk.as_ref() };
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data_ptr = unsafe { LocalChunk::<A>::data_ptr(chunk) };
        // Byte data has `align_of::<u8>() == 1`, so the chunk's
        // `CHUNK_ALIGN`-aligned payload base is trivially aligned for
        // the copy. The provider's `acquire_local(len)` postcondition
        // guarantees `capacity >= len`.
        let dest: *mut u8 = data_ptr.as_ptr();
        // `alloc_str_oversized` is only reachable for `len > max_normal_alloc`,
        // so `len > 0` is an invariant here.
        debug_assert!(len > 0, "alloc_str_oversized only reachable for len > max_normal_alloc > 0");
        // SAFETY: source and destination are both valid for `len`
        // bytes; the destination range was just reserved by
        // `acquire_local` and is exclusively owned by this call;
        // `&str` and the chunk payload cannot overlap because the
        // chunk was freshly returned from the underlying allocator.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), dest, len) };
        self.charge_alloc_stats(len);

        // Pin the chunk for the returned `&mut str`; reconciliation
        // leaves the pin's `+1` behind.
        let head = self.pinned_local.replace(None);
        chunk_ref.next.set(head);
        self.pinned_local.set(Some(chunk));
        // SAFETY: chunk held LARGE while we acted as its sole tenant;
        // `rcs_issued = 0`, `pinned = true` leaves +1 for the pin.
        unsafe { LocalChunk::reconcile_swap_out(chunk, 0, true) };

        // SAFETY: `dest` is non-null inside the chunk payload, the
        // chunk is pinned for the lifetime of `&self`, and `len` bytes
        // were just initialized from the source `&str`.
        let slice = unsafe { core::slice::from_raw_parts_mut(dest, len) };
        // SAFETY: the bytes were copied verbatim from a valid `&str`,
        // so the UTF-8 invariant is preserved.
        Ok(unsafe { core::str::from_utf8_unchecked_mut(slice) })
    }

    /// Panicking sibling of [`Self::try_alloc_str_inner`].
    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "see try_alloc_str_inner; both wrappers delegate to impl_alloc_str_inner which must fold PANIC"
    )]
    fn alloc_str_inner_or_panic(&self, s: &str) -> &mut str {
        // Under `PANIC = true`, `impl_alloc_str_inner` cannot return `Err`.
        expect_alloc(self.impl_alloc_str_inner::<true>(s))
    }

    fn try_alloc_str_prefixed_local(&self, s: &str, flavor: AllocFlavor) -> Result<NonNull<u8>, AllocError> {
        let bytes = s.as_bytes();
        let len = bytes.len();
        let total = core::mem::size_of::<usize>().checked_add(len).ok_or(AllocError)?;
        if total > self.provider.max_normal_alloc {
            debug_assert!(matches!(flavor, AllocFlavor::Rc | AllocFlavor::Box));
            // SAFETY: bytes is a valid slice of `len` u8s.
            return unsafe { self.try_alloc_prefixed_local_oversized::<u8>(bytes.as_ptr(), len, len) };
        }
        try_alloc_prefixed!(
            self = self,
            src_ptr = bytes.as_ptr(),
            len = len,
            payload_bytes = len,
            elem_ty = u8,
            slot = current_local,
            refill = refill_local,
            accounting = {
                // SAFETY: callers only invoke with `Rc` or `Box`; `SimpleRef`
                // strings go through a different code path.
                debug_assert!(matches!(flavor, AllocFlavor::Rc | AllocFlavor::Box));
                self.current_local.bump_smart_pointers_issued();
            },
        )
    }

    /// `OwnedInLocalChunk`-returning sibling of [`Self::try_alloc_str_prefixed_local`].
    /// Wraps the raw `NonNull<u8>` in `OwnedInLocalChunk` so consumer
    /// wrappers can construct `RcStr` / `BoxStr` via the **safe**
    /// `from_owned_in_chunk` instead of unsafe `from_raw_data`.
    fn try_alloc_str_prefixed_local_owned(&self, s: &str, flavor: AllocFlavor) -> Result<OwnedInLocalChunk<u8, A>, AllocError> {
        let raw = self.try_alloc_str_prefixed_local(s, flavor)?;
        // SAFETY: helper just produced a Local-chunk-resident length-prefixed
        // buffer with one `+1` reserved for the caller.
        Ok(unsafe { OwnedInLocalChunk::from_raw_alloc(raw) })
    }

    fn try_alloc_str_prefixed_shared(&self, s: &str) -> Result<NonNull<u8>, AllocError> {
        let bytes = s.as_bytes();
        let len = bytes.len();
        let total = core::mem::size_of::<usize>().checked_add(len).ok_or(AllocError)?;
        if total > self.provider.max_normal_alloc {
            // SAFETY: bytes is a valid slice of `len` u8s.
            return unsafe { self.try_alloc_prefixed_shared_oversized::<u8>(bytes.as_ptr(), len, len) };
        }
        try_alloc_prefixed!(
            self = self,
            src_ptr = bytes.as_ptr(),
            len = len,
            payload_bytes = len,
            elem_ty = u8,
            slot = current_shared,
            refill = refill_shared,
            accounting = {
                self.current_shared.bump_smart_pointers_issued();
            },
        )
    }

    /// `OwnedInSharedChunk`-returning sibling of
    /// [`Self::try_alloc_str_prefixed_shared`]. See
    /// [`Self::try_alloc_str_prefixed_local_owned`] for the rationale.
    fn try_alloc_str_prefixed_shared_owned(&self, s: &str) -> Result<OwnedInSharedChunk<u8, A>, AllocError> {
        let raw = self.try_alloc_str_prefixed_shared(s)?;
        // SAFETY: helper just produced a Shared-chunk-resident length-prefixed
        // buffer with one `+1` reserved for the caller.
        Ok(unsafe { OwnedInSharedChunk::from_raw_alloc(raw) })
    }

    /// Fallible variant of [`Self::alloc_str_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    #[expect(
        clippy::inline_always,
        reason = "callers supply constant `sharing`; const-folding requires inlining"
    )]
    #[inline(always)]
    pub fn try_alloc_str_rc(&self, s: impl AsRef<str>) -> Result<RcStr<A>, AllocError> {
        let owned = self.try_alloc_str_prefixed_local_owned(s.as_ref(), AllocFlavor::Rc)?;
        Ok(RcStr::from_owned_in_chunk(owned))
    }

    /// Fallible variant of [`Self::alloc_str_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    #[expect(
        clippy::inline_always,
        reason = "callers supply constant `sharing`; const-folding requires inlining"
    )]
    #[inline(always)]
    pub fn try_alloc_str_arc(&self, s: impl AsRef<str>) -> Result<ArcStr<A>, AllocError>
    where
        A: Send + Sync,
    {
        let owned = self.try_alloc_str_prefixed_shared_owned(s.as_ref())?;
        Ok(ArcStr::from_owned_in_chunk(owned))
    }

    /// Fallible variant of [`Self::alloc_str_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    #[expect(
        clippy::inline_always,
        reason = "callers supply constant `sharing`; const-folding requires inlining"
    )]
    #[inline(always)]
    pub fn try_alloc_str_box(&self, s: impl AsRef<str>) -> Result<BoxStr<A>, AllocError> {
        let owned = self.try_alloc_str_prefixed_local_owned(s.as_ref(), AllocFlavor::Box)?;
        Ok(BoxStr::from_owned_in_chunk(owned))
    }
}
