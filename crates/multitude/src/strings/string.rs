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

use crate::Arena;
use crate::strings::string_common::impl_arena_string_common;

/// A growable, mutable UTF-8 string that lives in an [`Arena`].
///
/// `String` is a **transient builder**: 32 bytes on 64-bit (data pointer +
/// length + capacity + arena reference). Its purpose is to be filled and
/// then frozen via [`Self::into_arena_box_str`] into a compact, immutable
/// [`Box<str>`](crate::Box) (8 bytes).
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
/// let frozen = s.into_arena_box_str();
/// assert_eq!(&*frozen, "hello, world!");
/// ```
pub struct String<'a, A: Allocator + Clone = Global> {
    pub(super) inner: crate::vec::Vec<'a, u8, A>,
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
    pub fn from_str_in(s: &str, arena: &'a Arena<A>) -> Self {
        let mut out = Self::with_capacity_in(s.len(), arena);
        out.push_str(s);
        out
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
    pub fn insert(&mut self, idx: usize, ch: char) {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        self.insert_str(idx, s);
    }

    /// Insert a string slice at byte index `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is greater than `self.len()`, if `idx` is not
    /// on a UTF-8 character boundary, if the resulting length would
    /// overflow `usize`, or if the backing allocator fails on growth.
    pub fn insert_str(&mut self, idx: usize, s: &str) {
        let len = self.inner.len();
        assert!(idx <= len, "insertion index out of bounds (was {idx}, len = {len})");
        assert!(self.as_str().is_char_boundary(idx), "insert_str: idx is not on a char boundary");
        let bytes = s.as_bytes();
        let added = bytes.len();
        if added == 0 {
            return;
        }
        self.inner.reserve(added);
        // Append the new bytes at the end (the buffer grew by `added`),
        // then rotate the region [idx..len+added] right by `added` so
        // the layout becomes [prefix ++ bytes ++ old_suffix].
        for &b in bytes {
            self.inner.push(b);
        }
        let region = &mut self.inner.as_mut_slice()[idx..len + added];
        region.rotate_right(added);
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
            inner: &'g mut crate::vec::Vec<'a, u8, A>,
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
    /// `usize`, or the backing allocator fails on growth.
    pub fn replace_range<R: RangeBounds<usize>>(&mut self, range: R, replace_with: &str) {
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

        // Rebuild via a staging vec to keep this fully safe.
        let mut staging: allocator_api2::vec::Vec<u8> = allocator_api2::vec::Vec::with_capacity(start + replace_with.len() + (len - end));
        staging.extend_from_slice(&self.as_bytes()[..start]);
        staging.extend_from_slice(replace_with.as_bytes());
        staging.extend_from_slice(&self.as_bytes()[end..]);
        self.inner.clear();
        self.inner.extend_from_slice(staging.as_slice());
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

    /// Freeze into an owned, mutable [`Box<str, A>`](crate::Box).
    ///
    /// **O(n)** — copies the bytes into a compact, length-prefixed
    /// allocation in the arena's shared chunks and produces an owned
    /// [`Box<str, A>`](crate::Box) (8 bytes) whose `Drop` releases
    /// the chunk hold. The copy is the deliberate trade-off for
    /// `Box<str, A>` being a `Send`-safe, atomically-refcounted single
    /// pointer that can outlive the arena.
    #[must_use]
    pub fn into_arena_box_str(self) -> crate::Box<str, A> {
        self.inner.arena().alloc_str_box(self.as_str())
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

impl<A: Allocator + Clone> PartialEq<&str> for String<'_, A> {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
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
impl<'a, A: Allocator + Clone> crate::vec::FromIteratorIn<char> for String<'a, A> {
    type Allocator = &'a Arena<A>;

    fn from_iter_in<I: IntoIterator<Item = char>>(iter: I, allocator: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(allocator);
        s.extend(iter);
        s
    }
}

impl<'a, 'b, A: Allocator + Clone> crate::vec::FromIteratorIn<&'b str> for String<'a, A> {
    type Allocator = &'a Arena<A>;

    fn from_iter_in<I: IntoIterator<Item = &'b str>>(iter: I, allocator: &'a Arena<A>) -> Self {
        let mut s = Self::new_in(allocator);
        s.extend(iter);
        s
    }
}
