// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Ergonomic / string-specific impls for [`Arc<str, A>`](crate::Arc)
//! and [`Box<str, A>`](crate::Box).
//!
//! Adds inherent `as_str` accessors, comparison with bare `str` / `&str`,
//! the `Arc<str> ↔ Arc<[u8]>` zero-copy retag, and a string-flavoured
//! `serde::Serialize`. The generic `Arc<T>` / `Box<T>` impls already
//! cover `Clone`, `Drop`, `Deref<Target = str>`, `AsRef`, `Borrow`,
//! `Debug`, `Display`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash`,
//! `Pointer`, and `Unpin`.

use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use crate::{Arc, Box, Rc};

impl<A: Allocator + Clone> Arc<str, A> {
    /// Borrow as `&str`. Ergonomic alias for `&**self` (`Deref<Target = str>`).
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        self
    }
}

impl<A: Allocator + Clone> PartialEq<str> for Arc<str, A> {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<A: Allocator + Clone> PartialEq<&str> for Arc<str, A> {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<A: Allocator + Clone> serde::ser::Serialize for Arc<str, A> {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<A: Allocator + Clone> From<Arc<str, A>> for Arc<[u8], A> {
    /// Convert an [`Arc<str, A>`](crate::Arc) into an
    /// [`Arc<[u8], A>`](crate::Arc) without copying — the same UTF-8 bytes,
    /// viewed as `[u8]`. **O(1)**.
    #[inline]
    fn from(s: Arc<str, A>) -> Self {
        use core::mem::ManuallyDrop;
        let me = ManuallyDrop::new(s);
        // Use the raw chunk pointer rather than `me.as_str().as_ptr()`:
        // the `&str` Deref would narrow the borrow-stack tag and break
        // the new `Arc<[u8]>`'s chunk-header recovery on drop.
        let thin: NonNull<u8> = me.thin_ptr();
        // SAFETY: `thin` carries chunk-wide provenance, the stored byte length
        // becomes the `Arc<[u8]>` metadata, and `ManuallyDrop` transfers the
        // chunk reference.
        unsafe { Self::from_raw(thin) }
    }
}

impl<A: Allocator + Clone> Rc<str, A> {
    /// Borrow as `&str`. Ergonomic alias for `&**self` (`Deref<Target = str>`).
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        self
    }
}

impl<A: Allocator + Clone> PartialEq<str> for Rc<str, A> {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<A: Allocator + Clone> PartialEq<&str> for Rc<str, A> {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<A: Allocator + Clone> serde::ser::Serialize for Rc<str, A> {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<A: Allocator + Clone> From<Rc<str, A>> for Rc<[u8], A> {
    /// Convert an [`Rc<str, A>`](crate::Rc) into an [`Rc<[u8], A>`](crate::Rc)
    /// without copying. See [`From<Arc<str, A>> for Arc<[u8], A>`](crate::Arc).
    #[inline]
    fn from(s: Rc<str, A>) -> Self {
        use core::mem::ManuallyDrop;
        let me = ManuallyDrop::new(s);
        let thin: NonNull<u8> = me.thin_ptr();
        // SAFETY: `str` and `[u8]` share layout and metadata (the `usize` byte
        // length) and the non-atomic strong-count prefix; `ManuallyDrop`
        // transfers the chunk `+1` to the new `Rc<[u8]>` and `thin_ptr` keeps
        // chunk-wide provenance.
        unsafe { Self::from_raw(thin) }
    }
}

impl<A: Allocator + Clone> Box<str, A> {
    /// Borrow as `&str`. Ergonomic alias for `&**self` (`Deref<Target = str>`).
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        self
    }

    /// Borrow as `&mut str`. Ergonomic alias for `&mut **self`
    /// (`DerefMut<Target = str>`).
    #[must_use]
    #[inline]
    pub fn as_mut_str(&mut self) -> &mut str {
        self
    }
}

impl<A: Allocator + Clone> From<Box<str, A>> for Box<[u8], A> {
    /// Convert a [`Box<str, A>`](crate::Box) into a
    /// [`Box<[u8], A>`](crate::Box) without copying — the same UTF-8 bytes,
    /// viewed as `[u8]`. **O(1)**.
    #[inline]
    fn from(s: Box<str, A>) -> Self {
        use core::mem::ManuallyDrop;
        let me = ManuallyDrop::new(s);
        let thin: NonNull<u8> = me.thin_ptr();
        // SAFETY: `str` and `[u8]` share layout and metadata (the `usize` byte
        // length); `Box` carries no strong count. `ManuallyDrop` transfers the
        // chunk `+1` to the new `Box<[u8]>` and `thin_ptr` keeps chunk-wide
        // provenance.
        unsafe { Self::from_raw(thin) }
    }
}

impl<A: Allocator + Clone> PartialEq<str> for Box<str, A> {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<A: Allocator + Clone> PartialEq<&str> for Box<str, A> {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<A: Allocator + Clone> serde::ser::Serialize for Box<str, A> {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
