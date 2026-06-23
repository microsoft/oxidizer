// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reference-counted leaf types, allocated in an [`Arena`].
//!
//! Each is a thin 8-byte newtype over a `multitude::Arc` (8 bytes even for DST
//! payloads), can outlive the arena, and offers a `&Arena` constructor.

use core::ops::Deref;

use multitude::{Arc, Arena};

/// An immutable, reference-counted binary blob (`Arc<[u8]>`), 8 bytes.
pub(crate) struct RcBinary(Arc<[u8]>);

impl RcBinary {
    /// Copies `bytes` into `arena`.
    #[must_use]
    pub(crate) fn new(arena: &Arena, bytes: &[u8]) -> Self {
        Self(arena.alloc_slice_copy_arc(bytes))
    }
}

impl Deref for RcBinary {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

/// An immutable, reference-counted UTF-8 string (`Arc<str>`), 8 bytes.
pub(crate) struct RcStr(Arc<str>);

impl RcStr {
    /// Copies `s` into `arena`.
    #[must_use]
    pub(crate) fn new(arena: &Arena, s: &str) -> Self {
        Self(arena.alloc_str_arc(s))
    }
}

impl Deref for RcStr {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

/// An immutable, reference-counted UTF-16 string (`ArcUtf16Str`), 8 bytes.
///
/// Like [`RcStr`] but transcoded to UTF-16 (handy at FFI / Windows boundaries);
/// still a thin 8-byte handle, with the `u16` length held in the chunk prefix.
pub(crate) struct RcUtf16Str(multitude::strings::ArcUtf16Str);

impl RcUtf16Str {
    /// Transcodes `s` to UTF-16 and copies it into `arena`.
    #[must_use]
    pub(crate) fn new(arena: &Arena, s: &str) -> Self {
        Self(arena.alloc_utf16_str_arc_from_str(s))
    }

    /// Length in UTF-16 code units (`u16` elements).
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    /// True iff the string is empty.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// An immutable, reference-counted slim array (`Arc<[T]>`), 8 bytes.
pub(crate) struct RcArray<T>(Arc<[T]>);

impl<T: Send + Sync> RcArray<T> {
    /// Materializes `items` into `arena` and freezes them. The iterator's exact
    /// length sizes the allocation precisely, so the freeze never reallocates.
    #[must_use]
    pub(crate) fn new<I>(arena: &Arena, items: I) -> Self
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let iter = items.into_iter();
        let mut vec = arena.alloc_vec_with_capacity(iter.len());
        for item in iter {
            vec.push(item);
        }
        Self(vec.try_into_arc().expect("arena allocation cannot fail in this example"))
    }
}

impl<T> Deref for RcArray<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}
