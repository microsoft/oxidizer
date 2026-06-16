// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::missing_panics_doc,
    clippy::undocumented_unsafe_blocks,
    reason = "reachable panics are programmer error, and unsafe code follows the documented `data`/`len`/`cap` invariants"
)]

use core::borrow::{Borrow, BorrowMut};
use core::cmp::Ordering;
use core::fmt::{self, Debug, Display, Formatter};
use core::hash::{Hash, Hasher};
use core::ops::{Bound, Deref, DerefMut, RangeBounds};
use core::str;

use allocator_api2::alloc::{AllocError, Allocator, Global};

use crate::strings::string_common::impl_arena_string_common;
use crate::vec::{FromIteratorIn, Vec};
use crate::{Arc, Arena, Box, FromIn};

/// A growable, mutable UTF-8 string that lives in an [`Arena`].
///
/// # Example
///
/// ```
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let mut s = arena.alloc_string();
/// s.push_str("hello, ");
/// s.push_str("world!");
/// assert_eq!(s.as_str(), "hello, world!");
/// let frozen = s.into_boxed_str();
/// assert_eq!(&*frozen, "hello, world!");
/// ```
pub struct String<'a, A: Allocator + Clone = Global> {
    pub(super) inner: Vec<'a, u8, A>,
}

impl<'a, A: Allocator + Clone> String<'a, A> {
    /// Returns a string slice view of this string's contents.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // SAFETY: the only way bytes enter `self.inner` is via `push_str` /
        // `push` / `write_str` / `write_char`, each of which appends a
        // well-formed UTF-8 sub-sequence. The UTF-8 invariant therefore
        // holds for the whole buffer.
        unsafe { str::from_utf8_unchecked(self.inner.as_slice()) }
    }

    /// Return the bytes view of this string.
    #[must_use]
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        self.inner.as_slice()
    }

    /// Return a mutable `str` view of this string.
    ///
    /// Callers must preserve UTF-8 well-formedness; mutating the bytes
    /// in a way that produces invalid UTF-8 is undefined behavior, but
    /// only via the unsafe `str` APIs that allow byte-level edits.
    #[inline]
    pub fn as_mut_str(&mut self) -> &mut str {
        // SAFETY: same UTF-8 invariant as `as_str`: every byte was
        // appended as part of a well-formed UTF-8 sub-sequence.
        unsafe { str::from_utf8_unchecked_mut(self.inner.as_mut_slice()) }
    }

    /// Construct a `String` containing `s`, copied into `arena`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails.
    #[must_use]
    pub(crate) fn from_str_in(s: &str, arena: &'a Arena<A>) -> Self {
        let mut out = Self::with_capacity_in(s.len(), arena);
        out.push_str(s);
        out
    }

    /// Fallible variant of [`Self::from_str_in`].
    pub(crate) fn try_from_str_in(s: &str, arena: &'a Arena<A>) -> Result<Self, allocator_api2::alloc::AllocError> {
        let mut out = Self::try_with_capacity_in(s.len(), arena)?;
        out.try_push_str(s)?;
        Ok(out)
    }

    /// Remove the last character from the string and return it.
    ///
    /// Returns `None` if the string is empty.
    pub fn pop(&mut self) -> Option<char> {
        let ch = self.as_str().chars().next_back()?;
        let new_len = self.len() - ch.len_utf8();
        self.inner.truncate(new_len);
        Some(ch)
    }

    /// Shorten the string to `new_len` bytes.
    ///
    /// If `new_len >= self.len()`, this has no effect. Capacity is
    /// unchanged.
    ///
    /// # Panics
    ///
    /// Panics if `new_len` is not on a UTF-8 character boundary.
    pub fn truncate(&mut self, new_len: usize) {
        if new_len >= self.len() {
            return;
        }
        assert!(
            self.as_str().is_char_boundary(new_len),
            "truncate: new_len is not on a char boundary"
        );
        self.inner.truncate(new_len);
    }

    /// Insert a character at byte index `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()` or not on a UTF-8
    /// character boundary, or if the backing allocator fails on growth.
    /// Use [`Self::try_insert`] for a fallible variant.
    pub fn insert(&mut self, idx: usize, ch: char) {
        crate::arena::ExpectAlloc::expect_alloc(self.try_insert(idx, ch));
    }

    /// Fallible variant of [`Self::insert`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails on growth.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()` or not on a UTF-8
    /// character boundary.
    pub fn try_insert(&mut self, idx: usize, ch: char) -> Result<(), AllocError> {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        self.try_insert_str(idx, s)
    }

    /// Insert a string slice at byte index `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()`, if `idx` is not
    /// on a UTF-8 character boundary, if the resulting length would
    /// overflow `usize`, or if the backing allocator fails on growth.
    /// Use [`Self::try_insert_str`] for a fallible variant.
    pub fn insert_str(&mut self, idx: usize, s: &str) {
        crate::arena::ExpectAlloc::expect_alloc(self.try_insert_str(idx, s));
    }

    /// Fallible variant of [`Self::insert_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails on growth, or
    /// if the resulting length would overflow `usize`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()` or if `idx` is not on a
    /// UTF-8 character boundary.
    pub fn try_insert_str(&mut self, idx: usize, s: &str) -> Result<(), AllocError> {
        let len = self.inner.len();
        assert!(idx <= len, "insertion index out of bounds (was {idx}, len = {len})");
        assert!(self.as_str().is_char_boundary(idx), "insert_str: idx is not on a char boundary");
        let bytes = s.as_bytes();
        let added = bytes.len();
        if added == 0 {
            return Ok(());
        }
        self.inner.try_reserve(added)?;
        // Append the new bytes at the end (the buffer grew by `added`),
        // then rotate the region [idx..len+added] right by `added` so
        // the layout becomes [prefix ++ bytes ++ old_suffix].
        for &b in bytes {
            self.inner.push(b);
        }
        let region = &mut self.inner.as_mut_slice()[idx..len + added];
        region.rotate_right(added);
        Ok(())
    }

    /// Remove the character at byte index `idx` and return it.
    ///
    /// # Panics
    ///
    /// Panics if `idx >= self.len()` or `idx` is not on a UTF-8
    /// character boundary.
    pub fn remove(&mut self, idx: usize) -> char {
        let ch = self.as_str()[idx..].chars().next().expect("remove: idx out of bounds");
        let ch_len = ch.len_utf8();
        // Rotate the suffix [idx..len] left by ch_len, then truncate.
        let len = self.inner.len();
        let region = &mut self.inner.as_mut_slice()[idx..len];
        region.rotate_left(ch_len);
        self.inner.truncate(len - ch_len);
        ch
    }

    /// Retain only the characters for which `f` returns `true`, in
    /// order.
    #[cfg_attr(test, mutants::skip)] // `+= → *=` on counter ⇒ infinite loop
    pub fn retain<F: FnMut(char) -> bool>(&mut self, mut f: F) {
        // In-place compaction that matches `std::string::String::retain`'s
        // panic contract: if `f` panics, the guard commits the prefix
        // processed so far (retained chars shifted into place) and drops
        // the unprocessed tail, leaving the string valid UTF-8.
        struct Guard<'g, 'a, A: Allocator + Clone> {
            inner: &'g mut Vec<'a, u8, A>,
            idx: usize,
            del_bytes: usize,
        }
        impl<A: Allocator + Clone> Drop for Guard<'_, '_, A> {
            fn drop(&mut self) {
                // `u8` has no `Drop`, so this only lowers the length to the
                // retained, already-compacted prefix.
                self.inner.truncate(self.idx - self.del_bytes);
            }
        }

        let len = self.inner.len();

        let mut guard = Guard {
            inner: &mut self.inner,
            idx: 0,
            del_bytes: 0,
        };
        while guard.idx < len {
            // SAFETY: `guard.idx` always lands on a UTF-8 char boundary
            // (it advances by whole `char` lengths) and is `< len`, so the
            // tail is valid UTF-8 and has at least one char.
            let ch = unsafe { str::from_utf8_unchecked(&guard.inner.as_slice()[guard.idx..len]) }
                .chars()
                .next()
                .expect("idx < len guarantees a remaining char");
            let ch_len = ch.len_utf8();
            if f(ch) {
                if guard.del_bytes > 0 {
                    let dst = guard.idx - guard.del_bytes;
                    guard.inner.as_mut_slice().copy_within(guard.idx..guard.idx + ch_len, dst);
                }
            } else {
                guard.del_bytes += ch_len;
            }
            guard.idx += ch_len;
        }
        // Normal completion: `idx == len`; the guard truncates to
        // `len - del_bytes`, committing the retained bytes.
        drop(guard);
    }

    /// Replace the bytes in `range` with the contents of `replace_with`.
    ///
    /// # Panics
    ///
    /// Panics if either bound is out of range, the bounds are not on
    /// UTF-8 character boundaries, the resulting length would overflow
    /// `usize`, or the backing allocator fails on growth. Use
    /// [`Self::try_replace_range`] for a fallible variant.
    pub fn replace_range<R: RangeBounds<usize>>(&mut self, range: R, replace_with: &str) {
        crate::arena::ExpectAlloc::expect_alloc(self.try_replace_range(range, replace_with));
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
    /// Panics if either bound is out of range or the bounds are not on UTF-8
    /// character boundaries.
    pub fn try_replace_range<R: RangeBounds<usize>>(&mut self, range: R, replace_with: &str) -> Result<(), AllocError> {
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
        assert!(start <= end, "replace_range: start > end");
        assert!(end <= len, "replace_range: end > len");
        let s_ref = self.as_str();
        assert!(s_ref.is_char_boundary(start), "replace_range: start is not on a char boundary");
        assert!(s_ref.is_char_boundary(end), "replace_range: end is not on a char boundary");

        self.inner.try_replace_range_with_slice(start, end, replace_with.as_bytes())
    }

    /// Append a single character.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Use [`Self::try_push`] for a fallible
    /// variant.
    #[inline]
    pub fn push(&mut self, ch: char) {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        self.push_str(&*s);
    }

    /// Fallible variant of [`Self::push`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`](allocator_api2::alloc::AllocError) if the backing
    /// allocator fails.
    #[inline]
    pub fn try_push(&mut self, ch: char) -> Result<(), AllocError> {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        self.try_push_str(&*s)
    }

    /// Append a string slice.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Use [`Self::try_push_str`] for a fallible
    /// variant.
    #[inline]
    pub fn push_str(&mut self, s: impl AsRef<str>) {
        self.inner.extend_from_slice(s.as_ref().as_bytes());
    }

    /// Fallible variant of [`Self::push_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`](allocator_api2::alloc::AllocError) if the backing
    /// allocator fails.
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "the hot path through `try_push_str` is the bump-then-memcpy fast path; the cold grow branch is `#[inline(never)]` so the inlinable body is small"
    )]
    pub fn try_push_str(&mut self, s: impl AsRef<str>) -> Result<(), AllocError> {
        self.inner.try_extend_from_slice(s.as_ref().as_bytes())
    }

    /// Consume the `String`, returning the underlying byte vector. Mirrors
    /// [`std::string::String::into_bytes`].
    #[must_use]
    pub fn into_bytes(self) -> Vec<'a, u8, A> {
        self.inner
    }

    /// Returns a mutable reference to the underlying byte vector. Mirrors
    /// [`std::string::String::as_mut_vec`].
    ///
    /// # Safety
    ///
    /// The caller must ensure the bytes remain valid UTF-8 before the
    /// borrow ends; the `String` invariant is otherwise violated.
    #[must_use]
    pub unsafe fn as_mut_vec(&mut self) -> &mut Vec<'a, u8, A> {
        &mut self.inner
    }

    /// Split the string in two at byte index `at`, returning the tail.
    ///
    /// Returns `[at, len)` as a new `String` in the same arena and leaves
    /// `[0, at)` in `self`. Mirrors [`std::string::String::split_off`].
    ///
    /// # Panics
    ///
    /// Panics if `at` is not on a `char` boundary, or is past the end. Use
    /// [`Self::try_split_off`] for a fallible variant.
    #[must_use]
    pub fn split_off(&mut self, at: usize) -> Self {
        crate::arena::ExpectAlloc::expect_alloc(self.try_split_off(at))
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
    /// Panics if `at` is not on a `char` boundary, or is past the end.
    pub fn try_split_off(&mut self, at: usize) -> Result<Self, AllocError> {
        assert!(self.as_str().is_char_boundary(at), "String::split_off: `at` is not a char boundary");
        Ok(Self {
            inner: self.inner.try_split_off(at)?,
        })
    }

    /// Clone the bytes in `src` and append them to the end.
    ///
    /// `src` is a byte-index range into `self`. Mirrors
    /// [`std::string::String::extend_from_within`].
    ///
    /// # Panics
    ///
    /// Panics if the range is out of bounds or its bounds are not on `char`
    /// boundaries. Use [`Self::try_extend_from_within`] for a fallible variant.
    pub fn extend_from_within<R: RangeBounds<usize>>(&mut self, src: R) {
        crate::arena::ExpectAlloc::expect_alloc(self.try_extend_from_within(src));
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
    /// boundaries.
    pub fn try_extend_from_within<R: RangeBounds<usize>>(&mut self, src: R) -> Result<(), AllocError> {
        let len = self.len();
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
        let s_ref = self.as_str();
        assert!(s_ref.is_char_boundary(start), "extend_from_within: start is not on a char boundary");
        assert!(s_ref.is_char_boundary(end), "extend_from_within: end is not on a char boundary");
        self.inner.try_extend_from_within(start..end)
    }

    /// Remove the `char`s in byte range `range`, returning a draining iterator.
    ///
    /// Mirrors [`std::string::String::drain`].
    ///
    /// The drained range is removed immediately; the returned iterator yields
    /// the removed characters (it is also double-ended).
    ///
    /// # Panics
    ///
    /// Panics if `range`'s bounds are out of range or not on `char`
    /// boundaries.
    pub fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Drain<'_, 'a, A> {
        let len = self.len();
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
        let s_ref = self.as_str();
        assert!(s_ref.is_char_boundary(start), "drain: start is not on a char boundary");
        assert!(s_ref.is_char_boundary(end), "drain: end is not on a char boundary");
        Drain {
            inner: self.inner.drain(start..end),
        }
    }

    /// Freeze into an owned, mutable [`Box<str, A>`](crate::Box). Mirrors
    /// [`std::string::String::into_boxed_str`]; [`Box::from`] is the trait
    /// form.
    ///
    /// **O(n)** — copies the contents.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails. Use
    /// [`Self::try_into_boxed_str`] for a fallible variant.
    #[must_use]
    pub fn into_boxed_str(self) -> Box<str, A> {
        crate::arena::ExpectAlloc::expect_alloc(self.try_into_boxed_str())
    }

    /// Fallible variant of [`Self::into_boxed_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the underlying allocator fails.
    pub fn try_into_boxed_str(self) -> Result<Box<str, A>, AllocError> {
        self.inner.arena().try_alloc_str_box(self.as_str())
    }

    /// Freeze into a shared [`Arc<str, A>`](crate::Arc). [`Arc::from`] is the
    /// trait form.
    ///
    /// **O(n)** — copies the contents.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails. Use [`Self::try_into_arc_str`]
    /// for a fallible variant.
    #[must_use]
    pub fn into_arc_str(self) -> Arc<str, A>
    where
        A: Send + Sync,
    {
        crate::arena::ExpectAlloc::expect_alloc(self.try_into_arc_str())
    }

    /// Fallible variant of [`Self::into_arc_str`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the underlying allocator fails.
    pub fn try_into_arc_str(self) -> Result<Arc<str, A>, AllocError>
    where
        A: Send + Sync,
    {
        self.inner.arena().try_alloc_str_arc(self.as_str())
    }

    /// Consume the `String`, returning an arena-lifetime mutable string
    /// reference `&'a mut str`. Mirrors [`std::string::String::leak`].
    ///
    /// **O(1) and allocation-free**: reinterprets the existing UTF-8 buffer
    /// in place.
    #[must_use]
    pub fn leak(self) -> &'a mut str {
        let bytes = self.inner.leak();
        // SAFETY: `String` maintains the UTF-8 invariant over its bytes.
        unsafe { str::from_utf8_unchecked_mut(bytes) }
    }
}

/// Number of bytes in the UTF-8 sequence whose leading byte is `b0`.
const fn utf8_seq_len(b0: u8) -> usize {
    if b0 < 0x80 {
        1
    } else if b0 < 0xE0 {
        2
    } else if b0 < 0xF0 {
        3
    } else {
        4
    }
}

/// Draining iterator over a byte range of a [`String`], returned by
/// [`String::drain`]. Yields the removed [`char`]s (double-ended). The
/// arena-bound analog of [`std::string::Drain`].
pub struct Drain<'d, 'a, A: Allocator + Clone> {
    inner: crate::vec::Drain<'d, 'a, u8, A>,
}

impl<A: Allocator + Clone> fmt::Debug for Drain<'_, '_, A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Drain").finish_non_exhaustive()
    }
}

impl<A: Allocator + Clone> Iterator for Drain<'_, '_, A> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        let b0 = self.inner.next()?;
        let len = utf8_seq_len(b0);
        let mut buf = [b0, 0, 0, 0];
        for slot in buf.iter_mut().take(len).skip(1) {
            *slot = self.inner.next().expect("Drain holds valid UTF-8");
        }
        // SAFETY: `String::drain` validated the range on `char` boundaries, so
        // the drained bytes form well-formed UTF-8.
        unsafe { core::str::from_utf8_unchecked(&buf[..len]) }.chars().next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        // Each remaining `char` is 1–4 bytes.
        let bytes = self.inner.len();
        (bytes.div_ceil(4), Some(bytes))
    }
}

impl<A: Allocator + Clone> DoubleEndedIterator for Drain<'_, '_, A> {
    fn next_back(&mut self) -> Option<char> {
        let last = self.inner.next_back()?;
        let mut buf = [0_u8; 4];
        buf[3] = last;
        let mut n = 1;
        let mut b = last;
        // Pull continuation bytes (`0b10xxxxxx`) from the back until the
        // leading byte; the drained range is valid UTF-8 so this terminates.
        while b & 0xC0 == 0x80 {
            b = self.inner.next_back().expect("Drain holds valid UTF-8");
            n += 1;
            buf[4 - n] = b;
        }
        // SAFETY: see `next`.
        unsafe { core::str::from_utf8_unchecked(&buf[4 - n..]) }.chars().next()
    }
}

impl<A: Allocator + Clone> core::iter::FusedIterator for Drain<'_, '_, A> {}

impl<'a, A: Allocator + Clone> From<String<'a, A>> for Box<str, A> {
    /// Freeze a [`String`] into an immutable [`Box<str, A>`](crate::Box).
    /// Mirrors `std`'s `From<String> for Box<str>`.
    #[inline]
    fn from(s: String<'a, A>) -> Self {
        s.into_boxed_str()
    }
}

impl<'a, A: Allocator + Clone + Send + Sync> From<String<'a, A>> for Arc<str, A> {
    /// Freeze a [`String`] into a shared [`Arc<str, A>`](crate::Arc).
    /// Mirrors `std`'s `From<String> for Arc<str>`.
    #[inline]
    fn from(s: String<'a, A>) -> Self {
        s.into_arc_str()
    }
}

impl<A: Allocator + Clone> Deref for String<'_, A> {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl<A: Allocator + Clone> DerefMut for String<'_, A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut str {
        self.as_mut_str()
    }
}

impl<A: Allocator + Clone> AsRef<str> for String<'_, A> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<A: Allocator + Clone> AsMut<str> for String<'_, A> {
    fn as_mut(&mut self) -> &mut str {
        self.as_mut_str()
    }
}

impl<A: Allocator + Clone> Borrow<str> for String<'_, A> {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl<A: Allocator + Clone> BorrowMut<str> for String<'_, A> {
    fn borrow_mut(&mut self) -> &mut str {
        self.as_mut_str()
    }
}

impl<A: Allocator + Clone> Debug for String<'_, A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.as_str(), f)
    }
}

impl<A: Allocator + Clone> Display for String<'_, A> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self.as_str(), f)
    }
}

impl<A: Allocator + Clone> PartialEq for String<'_, A> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl_arena_string_common!(String, u8);

impl<A: Allocator + Clone> Ord for String<'_, A> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl<A: Allocator + Clone> Hash for String<'_, A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl<A: Allocator + Clone> PartialEq<str> for String<'_, A> {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<A: Allocator + Clone> PartialEq<String<'_, A>> for str {
    #[inline]
    fn eq(&self, other: &String<'_, A>) -> bool {
        self == other.as_str()
    }
}

impl<A: Allocator + Clone> PartialEq<&str> for String<'_, A> {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl<A: Allocator + Clone> PartialEq<String<'_, A>> for &str {
    #[inline]
    fn eq(&self, other: &String<'_, A>) -> bool {
        *self == other.as_str()
    }
}

impl<A: Allocator + Clone> PartialEq<alloc::borrow::Cow<'_, str>> for String<'_, A> {
    #[inline]
    fn eq(&self, other: &alloc::borrow::Cow<'_, str>) -> bool {
        self.as_str() == &**other
    }
}

impl<A: Allocator + Clone> PartialEq<String<'_, A>> for alloc::borrow::Cow<'_, str> {
    #[inline]
    fn eq(&self, other: &String<'_, A>) -> bool {
        &**self == other.as_str()
    }
}

impl<A: Allocator + Clone> Clone for String<'_, A> {
    fn clone(&self) -> Self {
        Self::from_str_in(self.as_str(), self.inner.arena)
    }
}

impl<A: Allocator + Clone> Extend<char> for String<'_, A> {
    fn extend<I: IntoIterator<Item = char>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        let (lo, _) = iter.size_hint();
        self.reserve(lo);
        for ch in iter {
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            self.push_str(s);
        }
    }
}

impl<'a, A: Allocator + Clone> Extend<&'a str> for String<'_, A> {
    fn extend<I: IntoIterator<Item = &'a str>>(&mut self, iter: I) {
        for s in iter {
            self.push_str(s);
        }
    }
}
#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<A: Allocator + Clone> serde::ser::Serialize for String<'_, A> {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'a, A: Allocator + Clone> FromIteratorIn<'a, char, A> for String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = char>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIteratorIn<'a, &'b str, A> for String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = &'b str>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIteratorIn<'a, &'b char, A> for String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = &'b char>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<'a, A: Allocator + Clone> FromIteratorIn<'a, alloc::string::String, A> for String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = alloc::string::String>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<'a, A: Allocator + Clone> FromIteratorIn<'a, alloc::boxed::Box<str>, A> for String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = alloc::boxed::Box<str>>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIteratorIn<'a, alloc::borrow::Cow<'b, str>, A> for String<'a, A> {
    fn from_iter_in<I: IntoIterator<Item = alloc::borrow::Cow<'b, str>>>(iter: I, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.extend(iter);
        s
    }
}

impl<I: core::slice::SliceIndex<str>, A: Allocator + Clone> core::ops::Index<I> for String<'_, A> {
    type Output = I::Output;
    #[inline]
    fn index(&self, index: I) -> &Self::Output {
        core::ops::Index::index(self.as_str(), index)
    }
}

impl<I: core::slice::SliceIndex<str>, A: Allocator + Clone> core::ops::IndexMut<I> for String<'_, A> {
    #[inline]
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        core::ops::IndexMut::index_mut(self.as_mut_str(), index)
    }
}

impl<A: Allocator + Clone> AsRef<[u8]> for String<'_, A> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[cfg(feature = "std")]
impl<A: Allocator + Clone> AsRef<std::ffi::OsStr> for String<'_, A> {
    #[inline]
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.as_str().as_ref()
    }
}

#[cfg(feature = "std")]
impl<A: Allocator + Clone> AsRef<std::path::Path> for String<'_, A> {
    #[inline]
    fn as_ref(&self) -> &std::path::Path {
        self.as_str().as_ref()
    }
}

impl<A: Allocator + Clone> core::ops::Add<&str> for String<'_, A> {
    type Output = Self;
    /// Concatenates a `&str` onto the end of this `String`. Mirrors
    /// `std`'s `Add<&str> for String`.
    #[inline]
    fn add(mut self, rhs: &str) -> Self {
        self.push_str(rhs);
        self
    }
}

impl<A: Allocator + Clone> core::ops::AddAssign<&str> for String<'_, A> {
    #[inline]
    fn add_assign(&mut self, rhs: &str) {
        self.push_str(rhs);
    }
}

impl<'b, A: Allocator + Clone> Extend<&'b char> for String<'_, A> {
    fn extend<I: IntoIterator<Item = &'b char>>(&mut self, iter: I) {
        for c in iter {
            self.push(*c);
        }
    }
}

impl<'b, A: Allocator + Clone> Extend<alloc::borrow::Cow<'b, str>> for String<'_, A> {
    fn extend<I: IntoIterator<Item = alloc::borrow::Cow<'b, str>>>(&mut self, iter: I) {
        for s in iter {
            self.push_str(&s);
        }
    }
}

impl<A: Allocator + Clone> Extend<alloc::string::String> for String<'_, A> {
    fn extend<I: IntoIterator<Item = alloc::string::String>>(&mut self, iter: I) {
        for s in iter {
            self.push_str(&s);
        }
    }
}

impl<A: Allocator + Clone> Extend<alloc::boxed::Box<str>> for String<'_, A> {
    fn extend<I: IntoIterator<Item = alloc::boxed::Box<str>>>(&mut self, iter: I) {
        for s in iter {
            self.push_str(&s);
        }
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIn<'a, &'b str, A> for String<'a, A> {
    /// Copy `value` into a fresh arena string. Mirrors `std`'s `From<&str>`.
    #[inline]
    fn from_in(value: &'b str, arena: &'a Arena<A>) -> Self {
        Self::from_str_in(value, arena)
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIn<'a, &'b mut str, A> for String<'a, A> {
    /// Copy `value` into a fresh arena string. Mirrors `std`'s `From<&mut str>`.
    #[inline]
    fn from_in(value: &'b mut str, arena: &'a Arena<A>) -> Self {
        Self::from_str_in(value, arena)
    }
}

impl<'a, A: Allocator + Clone> FromIn<'a, char, A> for String<'a, A> {
    /// Build a one-character arena string. Mirrors `std`'s `From<char>`.
    #[inline]
    fn from_in(value: char, arena: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(arena);
        s.push(value);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> FromIn<'a, alloc::borrow::Cow<'b, str>, A> for String<'a, A> {
    /// Copy a clone-on-write string into the arena. Mirrors `std`'s `From<Cow<str>>`.
    #[inline]
    fn from_in(value: alloc::borrow::Cow<'b, str>, arena: &'a Arena<A>) -> Self {
        Self::from_str_in(&value, arena)
    }
}

impl<'a, A: Allocator + Clone> FromIn<'a, alloc::boxed::Box<str>, A> for String<'a, A> {
    /// Copy a boxed string into the arena. Mirrors `std`'s `From<Box<str>>`.
    #[inline]
    fn from_in(value: alloc::boxed::Box<str>, arena: &'a Arena<A>) -> Self {
        Self::from_str_in(&value, arena)
    }
}

#[cfg(test)]
mod tests {
    use super::utf8_seq_len;

    /// Pins [`utf8_seq_len`] at every UTF-8 lead-byte class boundary. The
    /// drain decoder relies on these exact lengths, and the boundary
    /// comparisons (`<`) must not drift to `<=`: e.g. `0xE0` leads a 3-byte
    /// sequence, never a 2-byte one.
    #[test]
    fn utf8_seq_len_matches_every_class_boundary() {
        assert_eq!(utf8_seq_len(0x00), 1);
        assert_eq!(utf8_seq_len(0x7F), 1);
        assert_eq!(utf8_seq_len(0x80), 2);
        assert_eq!(utf8_seq_len(0xDF), 2);
        assert_eq!(utf8_seq_len(0xE0), 3);
        assert_eq!(utf8_seq_len(0xEF), 3);
        assert_eq!(utf8_seq_len(0xF0), 4);
        assert_eq!(utf8_seq_len(0xFF), 4);
    }
}
