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

use allocator_api2::alloc::{AllocError, Allocator};

use super::{Arena, ExpectAlloc};
use crate::{Arc, Box};

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
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[must_use]
    #[inline]
    pub fn alloc_str(&self, s: impl AsRef<str>) -> &mut str {
        (self.impl_alloc_str(s.as_ref())).expect_alloc()
    }

    /// Copy `s` into the arena and return an [`Arc<str, A>`](crate::Arc)
    /// pointing to it.
    ///
    /// `Arc<str, A>` is a thin 8-byte refcounted smart pointer to a
    /// length-prefixed UTF-8 payload in a shared chunk. Clone is
    /// O(1) via a single atomic refcount bump on the hosting chunk.
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
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc_str(&self, s: impl AsRef<str>) -> Result<&mut str, AllocError> {
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
        self.impl_alloc_prefixed_shared::<u8>(s.as_ref().as_bytes()).map(|ptr|
            // SAFETY: see `Self::alloc_str_arc`.
            unsafe { Arc::from_raw(ptr) })
    }

    /// Fallible variant of [`Self::alloc_str_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    #[inline]
    pub fn try_alloc_str_box(&self, s: impl AsRef<str>) -> Result<Box<str, A>, AllocError> {
        self.impl_alloc_prefixed_shared::<u8>(s.as_ref().as_bytes()).map(|ptr|
            // SAFETY: see `Self::alloc_str_arc`.
            unsafe { Box::from_raw(ptr) })
    }

    /// Closure-free fast path for `alloc_str` / `try_alloc_str`. Mirrors
    /// `impl_alloc_value`: a `const PANIC: bool` parameter monomorphizes
    /// the error arm to either `panic_alloc!()` or `?`. Because `u8` has
    /// no drop, there is no `needs_drop` branch — the body is the
    /// minimal bump + memcpy + UTF-8 retag.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline(always)]
    fn impl_alloc_str(&self, s: &str) -> Result<&mut str, AllocError> {
        let len = s.len();
        loop {
            if let Some(u) = self.try_reserve_local_bytes(len) {
                #[cfg(feature = "stats")]
                self.record_alloc(len);
                return Ok(u.init_copy_from_str(s));
            }
            if self.is_oversized_local(len) {
                let ptr = self.alloc_oversized_local_with(len, |mutator| {
                    let ticket = mutator.try_alloc_bytes(len).expect("dedicated oversized chunk sized to fit string");
                    #[cfg(feature = "stats")]
                    self.record_alloc(len);
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
            self.refill_local(len)?;
        }
    }
}
