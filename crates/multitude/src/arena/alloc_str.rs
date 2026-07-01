// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
//! `&str` allocation API on [`Arena`].
//!
//! Mirrors [`super::alloc_value`]: the public `alloc_str` / `try_alloc_str`
//! entry points dispatch into a single `#[inline(always)]` helper
//! parameterized by `const PANIC: bool`, so each public path
//! monomorphizes to a specialized body with the error arm folded
//! away (`panic_alloc!()` on `PANIC=true`, `Err(AllocError)` otherwise).
//!
//! `Box<str, A>` and `Arc<str, A>` share a single length-prefixed
//! chunk layout (`[usize len][utf8 bytes]`, prefix unaligned — see
//! [`super::alloc_prefixed`]); both are thin 8-byte smart pointers.

use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::{Arena, ExpectAlloc};
use crate::internal::thin_dst::{AtomicStrong, LocalStrong, Strong};
use crate::{Alloc, AllocError, Arc, Box, Rc};

impl<A: Allocator + Clone> Arena<A> {
    /// Bump-allocate a copy of `s` and return an owning string handle.
    ///
    /// The returned [`Alloc<str>`](Alloc)'s lifetime is tied to `&self`. Like
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
    /// let mut s = arena.alloc_str("hello");
    /// s.make_ascii_uppercase();
    /// assert_eq!(&*s, "HELLO");
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_str(&self, s: impl AsRef<str>) -> Alloc<'_, str> {
        self.impl_alloc_str(s.as_ref()).expect_alloc()
    }

    /// Copy `s` into the arena and return an [`Arc<str, A>`](crate::Arc)
    /// pointing to it.
    ///
    /// `Arc<str, A>` is a thin 8-byte refcounted smart pointer to the UTF-8
    /// data in the arena. Clone is **O(1)**.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails.
    /// Use [`Self::try_alloc_str_arc`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let arena = multitude::Arena::new();
    /// let s = arena.alloc_str_arc("hello");
    /// assert_eq!(&*s, "hello");
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_str_arc(&self, s: impl AsRef<str>) -> Arc<str, A>
    where
        A: Send + Sync,
    {
        self.try_alloc_str_arc(s).expect_alloc()
    }

    /// Copy `s` into the arena and return a [`Box<str, A>`](crate::Box) smart pointer.
    ///
    /// `Box<str, A>` is a thin 8-byte owned, mutable string. Compared
    /// to [`Self::alloc_str_arc`]: the box is `!Clone` (single-owner)
    /// but supports `&mut str` access and releases its chunk hold the
    /// moment it is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails.
    /// Use [`Self::try_alloc_str_box`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let arena = multitude::Arena::new();
    /// let mut s = arena.alloc_str_box("hello");
    /// s.make_ascii_uppercase();
    /// assert_eq!(&*s, "HELLO");
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_str_box(&self, s: impl AsRef<str>) -> Box<str, A> {
        self.try_alloc_str_box(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    #[inline]
    pub fn try_alloc_str(&self, s: impl AsRef<str>) -> Result<Alloc<'_, str>, AllocError> {
        self.impl_alloc_str(s.as_ref())
    }

    /// Fallible variant of [`Self::alloc_str_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    #[inline]
    pub fn try_alloc_str_arc(&self, s: impl AsRef<str>) -> Result<Arc<str, A>, AllocError>
    where
        A: Send + Sync,
    {
        self.impl_alloc_str_smart::<AtomicStrong>(s.as_ref())
    }

    /// Copy `s` into the arena and return an [`Rc<str, A>`](crate::Rc) — a
    /// non-atomic, single-thread thin string smart pointer.
    ///
    /// Like [`Self::alloc_str_arc`] but `Rc<str>` is `!Send`/`!Sync`, with
    /// cheaper (non-atomic) clone/drop and slightly tighter packing.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails.
    /// Use [`Self::try_alloc_str_rc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_str_rc(&self, s: impl AsRef<str>) -> Rc<str, A> {
        self.try_alloc_str_rc(s).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_str_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    #[inline]
    pub fn try_alloc_str_rc(&self, s: impl AsRef<str>) -> Result<Rc<str, A>, AllocError> {
        self.impl_alloc_str_smart::<LocalStrong>(s.as_ref())
    }

    /// Copy `s` (UTF-8 bytes) into a strong-prefixed chunk allocation and adopt
    /// the payload into `S`'s thin string smart pointer ([`Arc<str>`] or
    /// [`Rc<str>`]). The payload is laid out exactly like an `[u8]` slice
    /// (length prefix + bytes); the smart pointer reinterprets it as `str`.
    #[inline]
    fn impl_alloc_str_smart<S: Strong>(&self, s: &str) -> Result<S::Ptr<str, A>, AllocError> {
        let thin = self.impl_alloc_prefixed_shared_arc::<S, u8>(s.as_bytes())?;
        // SAFETY: `impl_alloc_prefixed_shared_arc` returns a thin pointer to a
        // `len`-prefixed UTF-8 byte payload whose chunk prefix holds a strong
        // count of 1 and whose chunk it took a `+1` on, all within the chunk's
        // first tile. `str` and `[u8]` share that storage layout, so adopting it
        // as `S::Ptr<str, A>` (which reads the byte length as the `str`
        // metadata) is sound — exactly `S::adopt`'s contract.
        Ok(unsafe { S::adopt::<str, A>(thin.cast::<u8>()) })
    }

    /// Fallible variant of [`Self::alloc_str_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    #[inline]
    pub fn try_alloc_str_box(&self, s: impl AsRef<str>) -> Result<Box<str, A>, AllocError> {
        self.impl_alloc_prefixed_shared::<u8>(s.as_ref().as_bytes()).map(|ptr|
            // SAFETY: `impl_alloc_prefixed_shared::<u8>` returns a thin payload
            // pointer to UTF-8 bytes copied into the arena, with the byte length
            // written into the metadata prefix and a fresh chunk `+1` adopted.
            // `str` and `[u8]` share that length-prefixed layout, so
            // `Box::from_raw` reconstructs an owning `Box<str>`.
            unsafe { Box::from_raw(ptr) })
    }

    /// Adopting wrapper over [`Self::alloc_str_raw`]: copies `s` into a fresh
    /// arena slot and takes ownership of it in an [`Alloc`].
    #[inline(always)]
    fn impl_alloc_str(&self, s: &str) -> Result<Alloc<'_, str>, AllocError> {
        let slot = self.alloc_str_raw(s)?;
        // SAFETY: `alloc_str_raw` returns the unique `&mut str` for a
        // freshly-written arena slot that the arena hands out exactly once and
        // never drops itself, so `Alloc` may adopt it and own its destructor.
        Ok(unsafe { Alloc::from_mut(slot) })
    }

    /// Closure-free fast path for `alloc_str` / `try_alloc_str`. Mirrors
    /// `impl_alloc_value`: a `const PANIC: bool` parameter monomorphizes
    /// the error arm to either `panic_alloc!()` or `?`. Because `u8` has
    /// no drop, there is no `needs_drop` branch — the body is the
    /// minimal bump + memcpy + UTF-8 retag.
    #[allow(
        clippy::mut_from_ref,
        reason = "internal helper hands out a fresh, disjoint arena slot per call; the returned &mut is wrapped in an owning Alloc by impl_alloc_str"
    )]
    #[inline(always)]
    fn alloc_str_raw(&self, s: &str) -> Result<&mut str, AllocError> {
        let len = s.len();
        loop {
            if let Some(u) = self.try_reserve_local_bytes(len) {
                return Ok(u.init_copy_from_str(s));
            }
            if self.is_oversized(len) {
                let ptr = self.alloc_oversized_local_with(len, |mutator| {
                    let ticket = mutator.try_alloc_bytes(len).expect("dedicated oversized chunk sized to fit string");
                    // `init_copy_from_str` returns `&mut str` bound to the
                    // mutator's borrow; we hand back a raw pointer + len so
                    // the lifetime can be re-attached to `&Arena` once the
                    // mutator is parked in `retired_local`.
                    let r: &mut str = ticket.init_copy_from_str(s);
                    NonNull::from(r)
                })?;
                // SAFETY: chunk retained in `retired_local` for `&self`;
                // the bytes are valid UTF-8 (copied from `s`).
                return Ok(unsafe { &mut *ptr.as_ptr() });
            }
            self.refill(len)?;
        }
    }
}
