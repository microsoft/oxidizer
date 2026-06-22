// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Utf16Str`] DST newtype that lets UTF-16 strings ride on the generic
//! `Arc<Utf16Str>` / `Box<Utf16Str>` smart pointers.
//!
//! `widestring::Utf16Str` cannot be used directly as the `T` in the generic
//! [`Arc<T>`](crate::Arc) / [`Box<T>`](crate::Box) smart pointers because it
//! does not implement [`ptr_meta::Pointee`] and the orphan rule prevents us
//! from adding that impl for a foreign type. [`Utf16Str`] is a thin,
//! `#[repr(transparent)]` veneer over `[u16]` that *does* derive `Pointee`
//! (its metadata is the `u16` element count, exactly like `[u16]`), so
//! `Arc<Utf16Str>` / `Box<Utf16Str>` reuse all of the generic smart-pointer
//! machinery while [`Deref`]-ing to `widestring::Utf16Str` for ergonomics.

use core::cmp::Ordering;
use core::fmt::{self, Debug, Display, Formatter};
use core::hash::{Hash, Hasher};
use core::ops::{Deref, DerefMut};

use widestring::Utf16Str as WideUtf16Str;

/// An immutable UTF-16 string slice stored in an
/// [`Arena`](crate::Arena).
#[repr(transparent)]
pub struct Utf16Str([u16]);

// SAFETY: `Utf16Str` is `#[repr(transparent)]` over `[u16]`, so it shares
// `[u16]`'s pointer metadata — the `usize` element count. This is exactly what
// `#[derive(ptr_meta::Pointee)]` would emit; we write it by hand so the `utf16`
// feature doesn't need `ptr_meta`'s `derive` feature.
unsafe impl ptr_meta::Pointee for Utf16Str {
    type Metadata = usize;
}

impl Utf16Str {
    /// Borrow as a [`widestring::Utf16Str`].
    #[inline]
    #[must_use]
    pub fn as_widestring_utf16_str(&self) -> &widestring::Utf16Str {
        // SAFETY: the payload is well-formed UTF-16 by construction.
        unsafe { WideUtf16Str::from_slice_unchecked(&self.0) }
    }

    /// Borrow as a mutable [`widestring::Utf16Str`].
    #[inline]
    #[must_use]
    pub fn as_mut_widestring_utf16_str(&mut self) -> &mut widestring::Utf16Str {
        // SAFETY: the payload is well-formed UTF-16 by construction, and the
        // `&mut self` borrow grants exclusive access.
        unsafe { WideUtf16Str::from_slice_unchecked_mut(&mut self.0) }
    }
}

impl Deref for Utf16Str {
    type Target = WideUtf16Str;
    #[inline]
    fn deref(&self) -> &WideUtf16Str {
        self.as_widestring_utf16_str()
    }
}

impl DerefMut for Utf16Str {
    #[inline]
    fn deref_mut(&mut self) -> &mut WideUtf16Str {
        self.as_mut_widestring_utf16_str()
    }
}

impl Debug for Utf16Str {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.as_widestring_utf16_str(), f)
    }
}

impl Display for Utf16Str {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self.as_widestring_utf16_str(), f)
    }
}

impl PartialEq for Utf16Str {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for Utf16Str {}

impl PartialOrd for Utf16Str {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Utf16Str {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl Hash for Utf16Str {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq<WideUtf16Str> for Utf16Str {
    #[inline]
    fn eq(&self, other: &WideUtf16Str) -> bool {
        self.as_widestring_utf16_str() == other
    }
}
