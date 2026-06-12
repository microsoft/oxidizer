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

use crate::{Arc, Box};

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
    /// [`Arc<[u8], A>`](crate::Arc) without copying.
    ///
    /// `Arc<str, A>` is a thin 8-byte pointer to a length-prefixed UTF-8
    /// payload in a shared chunk; this reads the length from the chunk
    /// prefix, reconstructs a `NonNull<[u8]>` over the same payload,
    /// and transfers the chunk +1 into the new `Arc<[u8], A>`. O(1),
    /// no copy.
    #[inline]
    fn from(s: Arc<str, A>) -> Self {
        use core::mem::ManuallyDrop;
        let me = ManuallyDrop::new(s);
        // Use the raw chunk pointer rather than `me.as_str().as_ptr()`:
        // the `&str` Deref would narrow the borrow-stack tag and break
        // the new `Arc<[u8]>`'s chunk-header recovery on drop.
        let thin: NonNull<u8> = me.thin_ptr();
        // SAFETY: `thin` was produced by
        // `Arena::impl_alloc_prefixed_shared::<u8>`; it carries
        // chunk-wide provenance, the prefix word stores the byte
        // length (which becomes the `Arc<[u8]>` metadata read on
        // demand), and `ManuallyDrop` transfers the chunk +1.
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
    /// [`Box<[u8], A>`](crate::Box).
    ///
    /// `Box<str, A>` is a thin 8-byte pointer to a length-prefixed
    /// UTF-8 payload in a shared chunk; this retags the slice element
    /// type from `str` to `[u8]` (the chunk prefix's `usize` length
    /// stays the same regardless). The chunk +1 transfers from the
    /// source `Box<str>` to the new `Box<[u8]>` (no copy).
    #[inline]
    fn from(s: Box<str, A>) -> Self {
        use core::mem::ManuallyDrop;
        let me = ManuallyDrop::new(s);
        let thin: NonNull<u8> = me.thin_ptr();
        // SAFETY: see `From<Arc<str, A>> for Arc<[u8], A>` above.
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
