// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::borrow::Cow;
use core::borrow::{Borrow, BorrowMut};
use core::cmp::Ordering;
use core::fmt::{self, Debug, Display, Formatter};
use core::hash::{Hash, Hasher};
use core::mem::ManuallyDrop;
use core::ops::{Bound, Deref, DerefMut, Index, IndexMut, RangeBounds};
use core::slice::SliceIndex;

use allocator_api2::alloc::{AllocError, Allocator, Global};
use widestring::Utf16Str;

use crate::arena::ExpectAlloc;
use crate::strings::string_common::impl_arena_string_common;
use crate::vec::{FromIteratorIn, Vec};
use crate::{Arc, Arena, Box, FromIn, Rc};

/// A growable, mutable UTF-16 string that lives in an [`Arena`].
///
/// # Example
///
/// ```
/// # #[cfg(feature = "utf16")] {
/// use multitude::Arena;
/// use widestring::utf16str;
///
/// let arena = Arena::new();
/// let mut s = arena.alloc_utf16_string();
/// s.push_str(utf16str!("hello, "));
/// s.push_str(utf16str!("world!"));
/// assert_eq!(s.as_utf16_str(), utf16str!("hello, world!"));
/// let frozen = s.into_boxed_utf16_str();
/// assert_eq!(&*frozen, utf16str!("hello, world!"));
/// # }
/// ```
pub struct Utf16String<'a, A: Allocator + Clone = Global> {
    inner: Vec<'a, u16, A>,
}

impl<'a, A: Allocator + Clone> Utf16String<'a, A> {
    /// Borrow as `&Utf16Str`.
    #[must_use]
    pub fn as_utf16_str(&self) -> &Utf16Str {
        // SAFETY: the only way `u16`s enter `self.inner` is via push paths
        // that append well-formed UTF-16 code unit sequences (either
        // already-validated `&Utf16Str` or `char::encode_utf16` output).
        unsafe { Utf16Str::from_slice_unchecked(self.inner.as_slice()) }
    }

    /// Return the `u16` slice view.
    #[must_use]
    #[inline]
    pub fn as_slice(&self) -> &[u16] {
        self.inner.as_slice()
    }

    /// Return a mutable `Utf16Str` view of this string.
    #[inline]
    pub fn as_mut_utf16_str(&mut self) -> &mut Utf16Str {
        // SAFETY: same UTF-16 well-formedness invariant as `as_utf16_str`.
        unsafe { Utf16Str::from_slice_unchecked_mut(self.inner.as_mut_slice()) }
    }

    /// Construct an `Utf16String` containing `s`, copied into `arena`.
    #[must_use]
    pub fn from_utf16_str_in(s: &Utf16Str, arena: &'a Arena<A>) -> Self {
        Self::try_from_utf16_str_in(s, arena).expect_alloc()
    }

    /// Fallible variant of [`Self::from_utf16_str_in`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    pub fn try_from_utf16_str_in(s: &Utf16Str, arena: &'a Arena<A>) -> Result<Self, AllocError> {
        let mut out = Self::try_with_capacity_in(s.len(), arena)?;
        out.try_push_str(s)?;
        Ok(out)
    }

    /// Construct an `Utf16String` by transcoding a `&str` into UTF-16,
    /// copied into `arena`.
    #[must_use]
    fn from_str_in(s: &str, arena: &'a Arena<A>) -> Self {
        let mut out = Self::with_capacity_in(s.len(), arena);
        out.push_from_str(s);
        out
    }

    /// Remove the last character from the string and return it.
    pub fn pop(&mut self) -> Option<char> {
        let ch = self.as_utf16_str().chars().next_back()?;
        let new_len = self.len() - ch.len_utf16();
        self.inner.truncate(new_len);
        Some(ch)
    }

    /// Shorten the string to `new_len` `u16` elements.
    ///
    /// # Panics
    ///
    /// Panics if `new_len` is not on a UTF-16 character boundary
    /// (i.e., it would split a surrogate pair).
    pub fn truncate(&mut self, new_len: usize) {
        if new_len >= self.len() {
            return;
        }
        assert!(
            self.as_utf16_str().is_char_boundary(new_len),
            "Utf16String::truncate: new_len {new_len} is not on a UTF-16 char boundary"
        );
        self.inner.truncate(new_len);
    }

    /// Append a single character.
    #[inline]
    pub fn push(&mut self, ch: char) {
        let mut buf = [0u16; 2];
        let units = ch.encode_utf16(&mut buf);
        self.inner.extend_from_slice(&*units);
    }

    /// Fallible variant of [`Self::push`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails on growth.
    #[inline]
    pub fn try_push(&mut self, ch: char) -> Result<(), AllocError> {
        let mut buf = [0u16; 2];
        let units = ch.encode_utf16(&mut buf);
        self.inner.try_extend_from_slice(&*units)
    }

    /// Append a `Utf16Str`-like value.
    #[inline]
    pub fn push_str(&mut self, s: impl AsRef<Utf16Str>) {
        self.inner.extend_from_slice(s.as_ref().as_slice());
    }

    /// Fallible variant of [`Self::push_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails on growth.
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "the hot path is bump-then-memcpy; the cold grow branch is `#[inline(never)]` so the inlinable body is small"
    )]
    pub fn try_push_str(&mut self, s: impl AsRef<Utf16Str>) -> Result<(), AllocError> {
        self.inner.try_extend_from_slice(s.as_ref().as_slice())
    }

    /// Append a `&str`-like value, transcoding it to UTF-16.
    #[inline]
    pub fn push_from_str(&mut self, s: impl AsRef<str>) {
        let s = s.as_ref();
        self.inner.reserve(s.len());
        for ch in s.chars() {
            let mut buf = [0u16; 2];
            let units = ch.encode_utf16(&mut buf);
            self.inner.extend_from_slice(&*units);
        }
    }

    /// Fallible variant of [`Self::push_from_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails on growth.
    pub fn try_push_from_str(&mut self, s: impl AsRef<str>) -> Result<(), AllocError> {
        let s = s.as_ref();
        self.inner.try_reserve(s.len())?;
        for ch in s.chars() {
            let mut buf = [0u16; 2];
            let units = ch.encode_utf16(&mut buf);
            self.inner.try_extend_from_slice(&*units)?;
        }
        Ok(())
    }

    /// Insert a character at element index `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()` or not on a UTF-16
    /// character boundary, or if the backing allocator fails on growth.
    pub fn insert(&mut self, idx: usize, ch: char) {
        self.try_insert(idx, ch).expect_alloc();
    }

    /// Fallible variant of [`Self::insert`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails on growth.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()` or not on a UTF-16
    /// character boundary.
    pub fn try_insert(&mut self, idx: usize, ch: char) -> Result<(), AllocError> {
        let mut buf = [0u16; 2];
        let units = ch.encode_utf16(&mut buf);
        self.try_insert_units(idx, units)
    }

    /// Insert a `Utf16Str` at element index `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()`, if `idx` is not
    /// on a UTF-16 character boundary, if the resulting length would
    /// overflow `usize`, or if the backing allocator fails on growth.
    /// Use [`Self::try_insert_utf16_str`] for a fallible variant.
    pub fn insert_utf16_str(&mut self, idx: usize, s: &Utf16Str) {
        self.try_insert_utf16_str(idx, s).expect_alloc();
    }

    /// Fallible variant of [`Self::insert_utf16_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails on growth, or
    /// if the resulting length would overflow `usize`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()` or if `idx` is not on a
    /// UTF-16 character boundary.
    pub fn try_insert_utf16_str(&mut self, idx: usize, s: &Utf16Str) -> Result<(), AllocError> {
        self.try_insert_units(idx, s.as_slice())
    }

    fn try_insert_units(&mut self, idx: usize, units: &[u16]) -> Result<(), AllocError> {
        let len = self.inner.len();
        assert!(
            idx <= len,
            "Utf16String::insert: insertion index out of bounds (was {idx}, len = {len})"
        );
        assert!(
            self.as_utf16_str().is_char_boundary(idx),
            "Utf16String::insert: idx {idx} is not on a UTF-16 char boundary"
        );
        let added = units.len();
        if added == 0 {
            return Ok(());
        }
        self.inner.try_reserve(added)?;
        for &u in units {
            self.inner.push(u);
        }
        let region = &mut self.inner.as_mut_slice()[idx..len + added];
        region.rotate_right(added);
        Ok(())
    }

    /// Remove the character at element index `idx` and return it.
    ///
    /// # Panics
    ///
    /// Panics if `idx >= self.len()` or `idx` is not on a UTF-16
    /// character boundary.
    pub fn remove(&mut self, idx: usize) -> char {
        let len = self.inner.len();
        assert!(idx < len, "Utf16String::remove: idx {idx} out of bounds (len = {len})");
        assert!(
            self.as_utf16_str().is_char_boundary(idx),
            "Utf16String::remove: idx {idx} is not on a UTF-16 char boundary"
        );
        let tail = &self.as_slice()[idx..];
        // SAFETY: invariant — the buffer is well-formed UTF-16 and `idx`
        // is on a char boundary, so the tail is also well-formed.
        let tail_str = unsafe { Utf16Str::from_slice_unchecked(tail) };
        let ch = tail_str.chars().next().expect("remove: idx out of bounds");
        let ch_len = ch.len_utf16();
        let region = &mut self.inner.as_mut_slice()[idx..len];
        region.rotate_left(ch_len);
        self.inner.truncate(len - ch_len);
        ch
    }

    /// Retain only the characters for which `f` returns `true`.
    ///
    /// # Panic safety
    ///
    /// Matches [`std::string::String::retain`] and
    /// [`String::retain`](crate::strings::String::retain): if `f` panics, the
    /// characters processed so far are committed (retained ones compacted into
    /// place) and the unprocessed tail is dropped, leaving `self` well-formed
    /// UTF-16. Edits happen in place — no transient allocation.
    #[allow(
        clippy::missing_panics_doc,
        reason = "the internal `.expect` guards a char-boundary invariant (`idx < len`) and is unreachable; closure-panic behaviour is documented under `# Panic safety`"
    )]
    pub fn retain<F: FnMut(char) -> bool>(&mut self, mut f: F) {
        struct Guard<'g, 'a, A: Allocator + Clone> {
            inner: &'g mut Vec<'a, u16, A>,
            idx: usize,
            del_units: usize,
        }
        impl<A: Allocator + Clone> Drop for Guard<'_, '_, A> {
            fn drop(&mut self) {
                // `u16` has no `Drop`, so this only lowers the length to the
                // retained, already-compacted prefix.
                self.inner.truncate(self.idx - self.del_units);
            }
        }

        let len = self.inner.len();

        let mut guard = Guard {
            inner: &mut self.inner,
            idx: 0,
            del_units: 0,
        };
        while guard.idx < len {
            // SAFETY: `guard.idx` always lands on a UTF-16 char boundary (it
            // advances by whole `char` widths) and is `< len`, so the tail is
            // well-formed UTF-16 with at least one char.
            let ch = unsafe { Utf16Str::from_slice_unchecked(&guard.inner.as_slice()[guard.idx..len]) }
                .chars()
                .next()
                .expect("idx < len guarantees a remaining char");
            let ch_len = ch.len_utf16();
            if f(ch) {
                let dst = guard.idx - guard.del_units;
                guard.inner.as_mut_slice().copy_within(guard.idx..guard.idx + ch_len, dst);
            } else {
                guard.del_units += ch_len;
            }
            guard.idx += ch_len;
        }
        // Normal completion: `idx == len`; the guard truncates to
        // `len - del_units`, committing the retained units.
        drop(guard);
    }

    /// Replace the elements in `range` with the contents of `replace_with`.
    ///
    /// # Panics
    ///
    /// Panics if either bound is out of range, the bounds are not on
    /// UTF-16 character boundaries, the resulting length would overflow
    /// `usize`, or the backing allocator fails on growth.
    pub fn replace_range<R: RangeBounds<usize>>(&mut self, range: R, replace_with: &Utf16Str) {
        self.try_replace_range(range, replace_with).expect_alloc();
    }

    /// Fallible variant of [`Self::replace_range`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails on growth, or
    /// if the resulting length would overflow `usize`. On error `self` is
    /// left unchanged.
    ///
    /// # Panics
    ///
    /// Panics if either bound is out of range or the bounds are not on
    /// UTF-16 character boundaries.
    pub fn try_replace_range<R: RangeBounds<usize>>(&mut self, range: R, replace_with: &Utf16Str) -> Result<(), AllocError> {
        let len = self.len();
        let start = match range.start_bound() {
            Bound::Included(&i) => i,
            Bound::Excluded(&i) => i.checked_add(1).expect("replace_range: start bound overflows usize"),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&i) => i.checked_add(1).expect("replace_range: end bound overflows usize"),
            Bound::Excluded(&i) => i,
            Bound::Unbounded => len,
        };
        assert!(start <= end, "Utf16String::replace_range: start > end");
        assert!(end <= len, "Utf16String::replace_range: end > len");
        let s_ref = self.as_utf16_str();
        assert!(
            s_ref.is_char_boundary(start),
            "Utf16String::replace_range: start is not on a UTF-16 char boundary"
        );
        assert!(
            s_ref.is_char_boundary(end),
            "Utf16String::replace_range: end is not on a UTF-16 char boundary"
        );

        self.inner.try_replace_range_with_slice(start, end, replace_with.as_slice())
    }

    /// Consume the string, returning the underlying `u16` vector. The
    /// `into_bytes` analog for UTF-16.
    #[must_use]
    pub fn into_vec(self) -> Vec<'a, u16, A> {
        self.inner
    }

    /// Returns a mutable reference to the underlying `u16` vector.
    ///
    /// # Safety
    ///
    /// The caller must keep the units well-formed UTF-16 before the borrow
    /// ends; the `Utf16String` invariant is otherwise violated.
    #[must_use]
    pub unsafe fn as_mut_vec(&mut self) -> &mut Vec<'a, u16, A> {
        &mut self.inner
    }

    /// Split the string in two at `u16` index `at`, returning the tail.
    ///
    /// Returns `[at, len)` as a new `Utf16String` in the same arena and leaves
    /// `[0, at)` in `self`. The UTF-8 [`String::split_off`](crate::strings::String::split_off) analog.
    ///
    /// # Panics
    ///
    /// Panics if `at` is not on a `char` boundary (i.e. would split a
    /// surrogate pair), or is past the end. Use [`Self::try_split_off`] for a
    /// fallible variant.
    #[must_use]
    pub fn split_off(&mut self, at: usize) -> Self {
        self.try_split_off(at).expect_alloc()
    }

    /// Fallible variant of [`Self::split_off`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails. On error `self`
    /// is left unchanged.
    ///
    /// # Panics
    ///
    /// Panics if `at` is not on a `char` boundary (i.e. would split a
    /// surrogate pair), or is past the end.
    pub fn try_split_off(&mut self, at: usize) -> Result<Self, AllocError> {
        assert!(
            self.as_utf16_str().is_char_boundary(at),
            "Utf16String::split_off: `at` is not a char boundary"
        );
        Ok(Self {
            inner: self.inner.try_split_off(at)?,
        })
    }

    /// Clone the `u16` units in `src` and append them to the end.
    ///
    /// `src` is an index range into `self`. The UTF-16 analog of
    /// [`String::extend_from_within`](crate::strings::String::extend_from_within).
    ///
    /// # Panics
    ///
    /// Panics if the range is out of bounds or its bounds are not on `char`
    /// boundaries (i.e. would split a surrogate pair). Use
    /// [`Self::try_extend_from_within`] for a fallible variant.
    pub fn extend_from_within<R: RangeBounds<usize>>(&mut self, src: R) {
        self.try_extend_from_within(src).expect_alloc();
    }

    /// Fallible variant of [`Self::extend_from_within`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails while reserving.
    ///
    /// # Panics
    ///
    /// Panics if the range is out of bounds or its bounds are not on `char`
    /// boundaries (i.e. would split a surrogate pair).
    pub fn try_extend_from_within<R: RangeBounds<usize>>(&mut self, src: R) -> Result<(), AllocError> {
        let len = self.inner.len();
        let start = match src.start_bound() {
            Bound::Included(&i) => i,
            Bound::Excluded(&i) => i.checked_add(1).expect("extend_from_within: start bound overflows usize"),
            Bound::Unbounded => 0,
        };
        let end = match src.end_bound() {
            Bound::Included(&i) => i.checked_add(1).expect("extend_from_within: end bound overflows usize"),
            Bound::Excluded(&i) => i,
            Bound::Unbounded => len,
        };
        assert!(start <= end, "extend_from_within: start > end");
        assert!(end <= len, "extend_from_within: end > len");
        let s_ref = self.as_utf16_str();
        assert!(s_ref.is_char_boundary(start), "extend_from_within: start is not on a char boundary");
        assert!(s_ref.is_char_boundary(end), "extend_from_within: end is not on a char boundary");
        self.inner.try_extend_from_within(start..end)
    }

    /// Freeze into an owned, mutable
    /// `Box<Utf16Str>`. [`Box::from`](crate::Box)
    /// is the trait form.
    ///
    /// Generally **O(1)** (reuses the existing storage with no copy), except in
    /// rare edge cases where it falls back to an **O(n)** element move.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails. Use
    /// [`Self::try_into_boxed_utf16_str`] for a fallible variant.
    #[must_use]
    pub fn into_boxed_utf16_str(self) -> Box<crate::strings::Utf16Str, A> {
        self.try_into_boxed_utf16_str().expect_alloc()
    }

    /// Fallible variant of [`Self::into_boxed_utf16_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the underlying allocator fails.
    pub fn try_into_boxed_utf16_str(self) -> Result<Box<crate::strings::Utf16Str, A>, AllocError> {
        // Freeze the backing `Vec<u16>` (zero-copy when it carries the freeze
        // prefix, else an O(n) move), then retag `[u16] → Utf16Str`.
        let units = ManuallyDrop::new(self.inner.try_into_boxed_slice()?);
        // SAFETY: the units are well-formed UTF-16 (`Utf16String`'s
        // invariant), and `Box<Utf16Str>` / `Box<[u16]>` share the identical
        // length-prefixed `[u16]` chunk layout. The chunk `+1` transfers from
        // `units` (kept from dropping) to the new `Box<Utf16Str>`; `thin_ptr`
        // keeps chunk-wide provenance and the payload is `u16`-aligned.
        Ok(unsafe { Box::<crate::strings::Utf16Str, A>::from_raw(units.thin_ptr()) })
    }

    /// Freeze into a shared `Arc<Utf16Str>`. [`Arc::from`](crate::Arc) is the trait form.
    ///
    /// Generally **O(1)** (reuses the existing storage with no copy), except in
    /// rare edge cases where it falls back to an **O(n)** element move.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails. Use
    /// [`Self::try_into_arc_utf16_str`] for a fallible variant.
    #[must_use]
    pub fn into_arc_utf16_str(self) -> Arc<crate::strings::Utf16Str, A>
    where
        A: Send + Sync,
    {
        self.try_into_arc_utf16_str().expect_alloc()
    }

    /// Fallible variant of [`Self::into_arc_utf16_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the underlying allocator fails.
    pub fn try_into_arc_utf16_str(self) -> Result<Arc<crate::strings::Utf16Str, A>, AllocError>
    where
        A: Send + Sync,
    {
        // Freeze the backing `Vec<u16>` (zero-copy when it carries the freeze
        // prefix, else an O(n) move), then retag `[u16] → Utf16Str`.
        let units = ManuallyDrop::new(self.inner.try_into_arc_slice()?);
        // SAFETY: the units are well-formed UTF-16 (`Utf16String`'s invariant),
        // and `Arc<Utf16Str>` / `Arc<[u16]>` share the identical chunk layout
        // (strong count + length prefix + `u16` payload). The strong count
        // initialized to 1 by the freeze is exactly what `Arc<Utf16Str>`
        // expects, and the chunk `+1` transfers from `units` (kept from
        // dropping) to the new `Arc<Utf16Str>`; `thin_ptr` keeps chunk-wide
        // provenance and the payload is `u16`-aligned.
        Ok(unsafe { Arc::<crate::strings::Utf16Str, A>::from_raw(units.thin_ptr()) })
    }

    /// Freeze into a non-atomic `Rc<Utf16Str>`. [`Rc::from`](crate::Rc) is the
    /// trait form.
    ///
    /// Generally **O(1)** (reuses the existing storage with no copy), except in
    /// rare edge cases where it falls back to an **O(n)** element move.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails. Use
    /// [`Self::try_into_rc_utf16_str`] for a fallible variant.
    #[must_use]
    pub fn into_rc_utf16_str(self) -> Rc<crate::strings::Utf16Str, A> {
        self.try_into_rc_utf16_str().expect_alloc()
    }

    /// Fallible variant of [`Self::into_rc_utf16_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the underlying allocator fails.
    pub fn try_into_rc_utf16_str(self) -> Result<Rc<crate::strings::Utf16Str, A>, AllocError> {
        // Freeze the backing `Vec<u16>` (zero-copy when it carries the freeze
        // prefix, else an O(n) move), then retag `[u16] → Utf16Str`.
        let units = ManuallyDrop::new(self.inner.try_into_rc_slice()?);
        // SAFETY: the units are well-formed UTF-16 (`Utf16String`'s invariant),
        // and `Rc<Utf16Str>` / `Rc<[u16]>` share the identical chunk layout
        // (strong count + length prefix + `u16` payload). The strong count
        // initialized to 1 by the freeze reads back as the non-atomic `u32` 1
        // that `Rc<Utf16Str>` expects, and the chunk `+1` transfers from `units`
        // (kept from dropping) to the new `Rc<Utf16Str>`; `thin_ptr` keeps
        // chunk-wide provenance and the payload is `u16`-aligned.
        Ok(unsafe { Rc::<crate::strings::Utf16Str, A>::from_raw(units.thin_ptr()) })
    }

    /// Remove the `char`s in the `u16` index range `range`, returning a
    /// draining iterator over them. The UTF-16 analog of
    /// [`String::drain`](crate::strings::String::drain).
    ///
    /// The drained range is removed immediately; the returned iterator yields
    /// the removed characters (it is also double-ended).
    ///
    /// # Panics
    ///
    /// Panics if `range`'s bounds are out of range or not on `char`
    /// boundaries (i.e. would split a surrogate pair).
    pub fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Utf16Drain<'_, 'a, A> {
        let len = self.inner.len();
        let start = match range.start_bound() {
            Bound::Included(&i) => i,
            Bound::Excluded(&i) => i.checked_add(1).expect("drain: start bound overflows usize"),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&i) => i.checked_add(1).expect("drain: end bound overflows usize"),
            Bound::Excluded(&i) => i,
            Bound::Unbounded => len,
        };
        assert!(start <= end, "drain: start > end");
        assert!(end <= len, "drain: end > len");
        let s_ref = self.as_utf16_str();
        assert!(s_ref.is_char_boundary(start), "drain: start is not on a char boundary");
        assert!(s_ref.is_char_boundary(end), "drain: end is not on a char boundary");
        Utf16Drain {
            inner: self.inner.drain(start..end),
        }
    }

    /// Consume the string, returning an arena-lifetime mutable reference
    /// `&'a mut Utf16Str`. Mirrors [`String::leak`](crate::strings::String::leak).
    ///
    /// **O(1) and allocation-free**: reinterprets the existing buffer in place.
    #[must_use]
    pub fn leak(self) -> &'a mut Utf16Str {
        let units = self.inner.leak();
        // SAFETY: `Utf16String` maintains the well-formed-UTF-16 invariant.
        unsafe { Utf16Str::from_slice_unchecked_mut(units) }
    }
}

/// Draining iterator over a `u16` index range of a [`Utf16String`], returned
/// by [`Utf16String::drain`]. Yields the removed [`char`]s (double-ended).
pub struct Utf16Drain<'d, 'a, A: Allocator + Clone> {
    inner: crate::vec::Drain<'d, 'a, u16, A>,
}

impl<A: Allocator + Clone> fmt::Debug for Utf16Drain<'_, '_, A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Utf16Drain").finish_non_exhaustive()
    }
}

impl<A: Allocator + Clone> Iterator for Utf16Drain<'_, '_, A> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        let u0 = self.inner.next()?;
        let decoded = if (0xD800..=0xDBFF).contains(&u0) {
            let u1 = self.inner.next().expect("Utf16Drain holds valid UTF-16");
            char::decode_utf16([u0, u1]).next()
        } else {
            char::decode_utf16([u0]).next()
        };
        Some(decoded.expect("non-empty").expect("Utf16Drain holds valid UTF-16"))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        // Each remaining `char` is 1–2 `u16` units.
        let units = self.inner.len();
        (units.div_ceil(2), Some(units))
    }
}

impl<A: Allocator + Clone> DoubleEndedIterator for Utf16Drain<'_, '_, A> {
    fn next_back(&mut self) -> Option<char> {
        let last = self.inner.next_back()?;
        // A trailing unit at a char boundary is either a BMP scalar or the
        // low surrogate completing a pair; never a lone high surrogate.
        let decoded = if (0xDC00..=0xDFFF).contains(&last) {
            let high = self.inner.next_back().expect("Utf16Drain holds valid UTF-16");
            char::decode_utf16([high, last]).next()
        } else {
            char::decode_utf16([last]).next()
        };
        Some(decoded.expect("non-empty").expect("Utf16Drain holds valid UTF-16"))
    }
}

impl<A: Allocator + Clone> core::iter::FusedIterator for Utf16Drain<'_, '_, A> {}

impl<'a, A: Allocator + Clone> From<Utf16String<'a, A>> for Box<crate::strings::Utf16Str, A> {
    /// Freeze a [`Utf16String`] into an immutable
    /// `Box<Utf16Str>`.
    #[inline]
    fn from(s: Utf16String<'a, A>) -> Self {
        s.into_boxed_utf16_str()
    }
}

impl<'a, A: Allocator + Clone + Send + Sync> From<Utf16String<'a, A>> for Arc<crate::strings::Utf16Str, A> {
    /// Freeze a [`Utf16String`] into a shared
    /// `Arc<Utf16Str>`.
    #[inline]
    fn from(s: Utf16String<'a, A>) -> Self {
        s.into_arc_utf16_str()
    }
}

impl<'a, A: Allocator + Clone> From<Utf16String<'a, A>> for Rc<crate::strings::Utf16Str, A> {
    /// Freeze a [`Utf16String`] into a non-atomic `Rc<Utf16Str>`.
    #[inline]
    fn from(s: Utf16String<'a, A>) -> Self {
        s.into_rc_utf16_str()
    }
}

impl<A: Allocator + Clone> Deref for Utf16String<'_, A> {
    type Target = Utf16Str;
    #[inline]
    fn deref(&self) -> &Utf16Str {
        self.as_utf16_str()
    }
}

impl<A: Allocator + Clone> DerefMut for Utf16String<'_, A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Utf16Str {
        self.as_mut_utf16_str()
    }
}

impl<A: Allocator + Clone> AsRef<Utf16Str> for Utf16String<'_, A> {
    fn as_ref(&self) -> &Utf16Str {
        self.as_utf16_str()
    }
}

impl<A: Allocator + Clone> AsMut<Utf16Str> for Utf16String<'_, A> {
    fn as_mut(&mut self) -> &mut Utf16Str {
        self.as_mut_utf16_str()
    }
}

impl<A: Allocator + Clone> Borrow<Utf16Str> for Utf16String<'_, A> {
    fn borrow(&self) -> &Utf16Str {
        self.as_utf16_str()
    }
}

impl<A: Allocator + Clone> BorrowMut<Utf16Str> for Utf16String<'_, A> {
    fn borrow_mut(&mut self) -> &mut Utf16Str {
        self.as_mut_utf16_str()
    }
}

impl<A: Allocator + Clone> Debug for Utf16String<'_, A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.as_utf16_str(), f)
    }
}

impl<A: Allocator + Clone> Display for Utf16String<'_, A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self.as_utf16_str(), f)
    }
}

impl<A: Allocator + Clone> PartialEq for Utf16String<'_, A> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl_arena_string_common!(Utf16String, u16);

impl<A: Allocator + Clone> Ord for Utf16String<'_, A> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl<A: Allocator + Clone> Hash for Utf16String<'_, A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_utf16_str().hash(state);
    }
}

impl<A: Allocator + Clone> PartialEq<Utf16Str> for Utf16String<'_, A> {
    #[inline]
    fn eq(&self, other: &Utf16Str) -> bool {
        self.as_utf16_str() == other
    }
}

impl<A: Allocator + Clone> PartialEq<Utf16String<'_, A>> for Utf16Str {
    #[inline]
    fn eq(&self, other: &Utf16String<'_, A>) -> bool {
        self == other.as_utf16_str()
    }
}

impl<A: Allocator + Clone> PartialEq<&Utf16Str> for Utf16String<'_, A> {
    #[inline]
    fn eq(&self, other: &&Utf16Str) -> bool {
        self.as_utf16_str() == *other
    }
}

impl<A: Allocator + Clone> PartialEq<Utf16String<'_, A>> for &Utf16Str {
    #[inline]
    fn eq(&self, other: &Utf16String<'_, A>) -> bool {
        *self == other.as_utf16_str()
    }
}

impl<A: Allocator + Clone> Clone for Utf16String<'_, A> {
    fn clone(&self) -> Self {
        Self::from_utf16_str_in(self.as_utf16_str(), self.inner.arena())
    }
}

impl<A: Allocator + Clone> Extend<char> for Utf16String<'_, A> {
    fn extend<I: IntoIterator<Item = char>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        let (lo, _) = iter.size_hint();
        self.reserve(lo);
        for ch in iter {
            self.push(ch);
        }
    }
}

impl<'a, A: Allocator + Clone> Extend<&'a Utf16Str> for Utf16String<'_, A> {
    fn extend<I: IntoIterator<Item = &'a Utf16Str>>(&mut self, iter: I) {
        for s in iter {
            self.push_str(s);
        }
    }
}

impl<'a, A: Allocator + Clone> Extend<&'a str> for Utf16String<'_, A> {
    fn extend<I: IntoIterator<Item = &'a str>>(&mut self, iter: I) {
        for s in iter {
            self.push_from_str(s);
        }
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<A: Allocator + Clone> serde::ser::Serialize for Utf16String<'_, A> {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self.as_utf16_str())
    }
}

impl<A: Allocator + Clone> fmt::Write for Utf16String<'_, A> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_from_str(s);
        Ok(())
    }
}

impl<'a, A: Allocator + Clone> FromIteratorIn<'a, char, A> for Utf16String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = char>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIteratorIn<'a, &'b Utf16Str, A> for Utf16String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = &'b Utf16Str>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIteratorIn<'a, &'b str, A> for Utf16String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = &'b str>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIteratorIn<'a, &'b char, A> for Utf16String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = &'b char>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<'a, A: Allocator + Clone> FromIteratorIn<'a, alloc::string::String, A> for Utf16String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = alloc::string::String>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<'a, A: Allocator + Clone> FromIteratorIn<'a, alloc::boxed::Box<str>, A> for Utf16String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = alloc::boxed::Box<str>>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIteratorIn<'a, Cow<'b, str>, A> for Utf16String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = Cow<'b, str>>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<A: Allocator + Clone> AsRef<[u16]> for Utf16String<'_, A> {
    #[inline]
    fn as_ref(&self) -> &[u16] {
        self.as_slice()
    }
}

impl<A: Allocator + Clone> core::ops::Add<&Utf16Str> for Utf16String<'_, A> {
    type Output = Self;
    #[inline]
    fn add(mut self, rhs: &Utf16Str) -> Self {
        self.push_str(rhs);
        self
    }
}

impl<A: Allocator + Clone> core::ops::AddAssign<&Utf16Str> for Utf16String<'_, A> {
    #[inline]
    fn add_assign(&mut self, rhs: &Utf16Str) {
        self.push_str(rhs);
    }
}

impl<I, A: Allocator + Clone> Index<I> for Utf16String<'_, A>
where
    I: RangeBounds<usize> + SliceIndex<[u16], Output = [u16]>,
{
    type Output = Utf16Str;
    #[inline]
    fn index(&self, index: I) -> &Utf16Str {
        Index::index(self.as_utf16_str(), index)
    }
}

impl<I, A: Allocator + Clone> IndexMut<I> for Utf16String<'_, A>
where
    I: RangeBounds<usize> + SliceIndex<[u16], Output = [u16]>,
{
    #[inline]
    fn index_mut(&mut self, index: I) -> &mut Utf16Str {
        IndexMut::index_mut(self.as_mut_utf16_str(), index)
    }
}

impl<'b, A: Allocator + Clone> Extend<&'b char> for Utf16String<'_, A> {
    fn extend<I: IntoIterator<Item = &'b char>>(&mut self, iter: I) {
        for c in iter {
            self.push(*c);
        }
    }
}

impl<'b, A: Allocator + Clone> Extend<Cow<'b, str>> for Utf16String<'_, A> {
    fn extend<I: IntoIterator<Item = Cow<'b, str>>>(&mut self, iter: I) {
        for s in iter {
            self.push_from_str(&s);
        }
    }
}

impl<A: Allocator + Clone> Extend<alloc::string::String> for Utf16String<'_, A> {
    fn extend<I: IntoIterator<Item = alloc::string::String>>(&mut self, iter: I) {
        for s in iter {
            self.push_from_str(&s);
        }
    }
}

impl<A: Allocator + Clone> Extend<alloc::boxed::Box<str>> for Utf16String<'_, A> {
    fn extend<I: IntoIterator<Item = alloc::boxed::Box<str>>>(&mut self, iter: I) {
        for s in iter {
            self.push_from_str(&s);
        }
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIn<'a, &'b Utf16Str, A> for Utf16String<'a, A> {
    /// Copy `value` into a fresh arena string. The `&Utf16Str` analog of
    /// `std`'s `From<&str> for String`.
    #[inline]
    fn from_in(value: &'b Utf16Str, arena: &'a Arena<A>) -> Self {
        Self::from_utf16_str_in(value, arena)
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIn<'a, &'b str, A> for Utf16String<'a, A> {
    /// Transcode a UTF-8 `&str` into a fresh arena UTF-16 string.
    #[inline]
    fn from_in(value: &'b str, arena: &'a Arena<A>) -> Self {
        Self::from_str_in(value, arena)
    }
}

impl<'a, A: Allocator + Clone> FromIn<'a, char, A> for Utf16String<'a, A> {
    /// Build a one-character arena string. Mirrors `std`'s `From<char>`.
    #[inline]
    fn from_in(value: char, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.push(value);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIn<'a, Cow<'b, Utf16Str>, A> for Utf16String<'a, A> {
    /// Copy a clone-on-write UTF-16 string into the arena.
    #[inline]
    fn from_in(value: Cow<'b, Utf16Str>, arena: &'a Arena<A>) -> Self {
        Self::from_utf16_str_in(&value, arena)
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIn<'a, Cow<'b, str>, A> for Utf16String<'a, A> {
    /// Transcode a clone-on-write UTF-8 string into the arena.
    #[inline]
    fn from_in(value: Cow<'b, str>, arena: &'a Arena<A>) -> Self {
        Self::from_str_in(&value, arena)
    }
}

impl<'a, A: Allocator + Clone> FromIn<'a, alloc::boxed::Box<str>, A> for Utf16String<'a, A> {
    /// Transcode a boxed UTF-8 string into the arena.
    #[inline]
    fn from_in(value: alloc::boxed::Box<str>, arena: &'a Arena<A>) -> Self {
        Self::from_str_in(&value, arena)
    }
}
