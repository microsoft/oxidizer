// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::undocumented_unsafe_blocks,
    reason = "internal docs mirror std-style APIs; unsafe code follows the documented `data`/`len`/`cap` invariants"
)]

use core::borrow::{Borrow, BorrowMut};
use core::cmp::Ordering;
use core::fmt::{self, Debug, Display, Formatter};
use core::hash::{Hash, Hasher};
use core::ops::{Bound, Deref, DerefMut, RangeBounds};

use allocator_api2::alloc::{AllocError, Allocator, Global};
use widestring::Utf16Str;

use crate::Arena;
use crate::strings::string_common::impl_arena_string_common;

/// A growable, mutable UTF-16 string that lives in an [`Arena`].
///
/// `Utf16String` is a **transient builder**: 32 bytes on 64-bit (data
/// pointer + length + capacity + arena reference). Lengths and
/// capacities are counted in `u16` elements.
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
/// let frozen = s.into_arena_box_utf16_str();
/// assert_eq!(&*frozen, utf16str!("hello, world!"));
/// # }
/// ```
pub struct Utf16String<'a, A: Allocator + Clone = Global> {
    pub(super) inner: crate::vec::Vec<'a, u16, A>,
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
        let mut out = Self::with_capacity_in(s.len(), arena);
        out.push_str(s);
        out
    }

    /// Construct an `Utf16String` by transcoding a `&str` into UTF-16,
    /// copied into `arena`.
    #[must_use]
    pub fn from_str_in(s: &str, arena: &'a Arena<A>) -> Self {
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
        let mut buf = [0u16; 2];
        let units = ch.encode_utf16(&mut buf);
        self.insert_units(idx, units);
    }

    /// Insert a `Utf16Str` at element index `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()`, if `idx` is not
    /// on a UTF-16 character boundary, if the resulting length would
    /// overflow `usize`, or if the backing allocator fails on growth.
    pub fn insert_utf16_str(&mut self, idx: usize, s: &Utf16Str) {
        self.insert_units(idx, s.as_slice());
    }

    fn insert_units(&mut self, idx: usize, units: &[u16]) {
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
            return;
        }
        self.inner.reserve(added);
        for &u in units {
            self.inner.push(u);
        }
        let region = &mut self.inner.as_mut_slice()[idx..len + added];
        region.rotate_right(added);
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
    /// If `f` panics, `self` is left **unchanged** (the original
    /// contents are preserved). This differs from
    /// [`crate::strings::String::retain`] which commits the
    /// already-processed prefix on panic. The difference is internal
    /// implementation detail: this variant uses a side buffer and
    /// only commits if the full pass completes without panicking,
    /// whereas the UTF-8 variant edits in place.
    ///
    /// # Allocator
    ///
    /// Allocates the side buffer from the **global** allocator, not
    /// the arena. Callers that require zero arena-foreign allocations
    /// in their hot path should avoid this method.
    pub fn retain<F: FnMut(char) -> bool>(&mut self, mut f: F) {
        let mut kept: allocator_api2::vec::Vec<u16> = allocator_api2::vec::Vec::with_capacity(self.len());
        for ch in self.as_utf16_str().chars() {
            if f(ch) {
                let mut buf = [0u16; 2];
                let units = ch.encode_utf16(&mut buf);
                kept.extend_from_slice(units);
            }
        }
        self.inner.clear();
        self.inner.extend_from_slice(kept.as_slice());
    }

    /// Replace the elements in `range` with the contents of `replace_with`.
    ///
    /// # Panics
    ///
    /// Panics if either bound is out of range, the bounds are not on
    /// UTF-16 character boundaries, the resulting length would overflow
    /// `usize`, or the backing allocator fails on growth.
    pub fn replace_range<R: RangeBounds<usize>>(&mut self, range: R, replace_with: &Utf16Str) {
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

        let mut staging: allocator_api2::vec::Vec<u16> = allocator_api2::vec::Vec::with_capacity(start + replace_with.len() + (len - end));
        staging.extend_from_slice(&self.as_slice()[..start]);
        staging.extend_from_slice(replace_with.as_slice());
        staging.extend_from_slice(&self.as_slice()[end..]);
        self.inner.clear();
        self.inner.extend_from_slice(staging.as_slice());
    }

    /// Freeze into an owned, mutable
    /// [`BoxUtf16Str<A>`](crate::strings::BoxUtf16Str).
    ///
    /// **O(n)** — copies the `u16` units into a compact, length-prefixed
    /// allocation in the arena's shared chunks and produces an owned
    /// [`BoxUtf16Str`](crate::strings::BoxUtf16Str) (8 bytes) whose
    /// `Drop` releases the chunk hold. The copy is the deliberate
    /// trade-off for `BoxUtf16Str` being a `Send`-safe,
    /// atomically-refcounted single pointer that can outlive the arena.
    #[must_use]
    pub fn into_arena_box_utf16_str(self) -> crate::strings::BoxUtf16Str<A> {
        self.inner.arena().alloc_utf16_str_box(self.as_utf16_str())
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

impl<A: Allocator + Clone> PartialEq<&Utf16Str> for Utf16String<'_, A> {
    #[inline]
    fn eq(&self, other: &&Utf16Str) -> bool {
        self.as_utf16_str() == *other
    }
}

impl<A: Allocator + Clone> Clone for Utf16String<'_, A> {
    fn clone(&self) -> Self {
        Self::from_utf16_str_in(self.as_utf16_str(), self.inner.arena)
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
        serializer.collect_str(&self.as_utf16_str().to_string())
    }
}

impl<A: Allocator + Clone> fmt::Write for Utf16String<'_, A> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_from_str(s);
        Ok(())
    }
}

impl<'a, A: Allocator + Clone> crate::vec::FromIteratorIn<char> for Utf16String<'a, A> {
    type Allocator = &'a Arena<A>;

    fn from_iter_in<I: IntoIterator<Item = char>>(iter: I, allocator: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(allocator);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> crate::vec::FromIteratorIn<&'b Utf16Str> for Utf16String<'a, A> {
    type Allocator = &'a Arena<A>;

    fn from_iter_in<I: IntoIterator<Item = &'b Utf16Str>>(iter: I, allocator: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(allocator);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> crate::vec::FromIteratorIn<&'b str> for Utf16String<'a, A> {
    type Allocator = &'a Arena<A>;

    fn from_iter_in<I: IntoIterator<Item = &'b str>>(iter: I, allocator: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(allocator);
        s.extend(iter);
        s
    }
}
