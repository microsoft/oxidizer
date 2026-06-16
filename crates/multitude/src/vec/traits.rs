// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Standard trait impls for [`Vec`].

use core::borrow::{Borrow, BorrowMut};
use core::cmp::Ordering;
use core::hash::{Hash, Hasher};
use core::ops::{Deref, DerefMut};
use core::{fmt, slice};

use allocator_api2::alloc::Allocator;

use super::{IntoIter, Vec};
use crate::Arena;

impl<T, A: Allocator + Clone> Deref for Vec<'_, T, A> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, A: Allocator + Clone> DerefMut for Vec<'_, T, A> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, A: Allocator + Clone> AsRef<[T]> for Vec<'_, T, A> {
    #[inline]
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, A: Allocator + Clone> AsMut<[T]> for Vec<'_, T, A> {
    #[inline]
    fn as_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, A: Allocator + Clone> Borrow<[T]> for Vec<'_, T, A> {
    #[inline]
    fn borrow(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, A: Allocator + Clone> BorrowMut<[T]> for Vec<'_, T, A> {
    #[inline]
    fn borrow_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, A: Allocator + Clone> Extend<T> for Vec<'_, T, A> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        self.reserve(lower);
        for item in iter {
            self.push(item);
        }
    }
}

impl<'a, 'b, T: Copy + 'b, A: Allocator + Clone> Extend<&'b T> for Vec<'a, T, A>
where
    'a: 'b,
{
    fn extend<I: IntoIterator<Item = &'b T>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        self.reserve(lower);
        for item in iter {
            self.push(*item);
        }
    }
}

impl<T: fmt::Debug, A: Allocator + Clone> fmt::Debug for Vec<'_, T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_slice(), f)
    }
}

// `PartialEq` mirrors `std::vec::Vec`'s cross-type matrix: comparison is by
// element (`T: PartialEq<U>`) and works in both directions against slices,
// arrays, `Cow`, and other `Vec`s (any allocator).
impl<'b, T: PartialEq<U>, U, A: Allocator + Clone, A2: Allocator + Clone> PartialEq<Vec<'b, U, A2>> for Vec<'_, T, A> {
    #[inline]
    fn eq(&self, other: &Vec<'b, U, A2>) -> bool {
        self[..] == other[..]
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone> PartialEq<[U]> for Vec<'_, T, A> {
    #[inline]
    fn eq(&self, other: &[U]) -> bool {
        self[..] == other[..]
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone> PartialEq<Vec<'_, U, A>> for [T] {
    #[inline]
    fn eq(&self, other: &Vec<'_, U, A>) -> bool {
        self[..] == other[..]
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone> PartialEq<&[U]> for Vec<'_, T, A> {
    #[inline]
    fn eq(&self, other: &&[U]) -> bool {
        self[..] == other[..]
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone> PartialEq<Vec<'_, U, A>> for &[T] {
    #[inline]
    fn eq(&self, other: &Vec<'_, U, A>) -> bool {
        self[..] == other[..]
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone> PartialEq<&mut [U]> for Vec<'_, T, A> {
    #[inline]
    fn eq(&self, other: &&mut [U]) -> bool {
        self[..] == other[..]
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone> PartialEq<Vec<'_, U, A>> for &mut [T] {
    #[inline]
    fn eq(&self, other: &Vec<'_, U, A>) -> bool {
        self[..] == other[..]
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone, const N: usize> PartialEq<[U; N]> for Vec<'_, T, A> {
    #[inline]
    fn eq(&self, other: &[U; N]) -> bool {
        self[..] == other[..]
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone, const N: usize> PartialEq<&[U; N]> for Vec<'_, T, A> {
    #[inline]
    fn eq(&self, other: &&[U; N]) -> bool {
        self[..] == other[..]
    }
}
impl<T: PartialEq<U> + Clone, U, A: Allocator + Clone> PartialEq<Vec<'_, U, A>> for alloc::borrow::Cow<'_, [T]> {
    #[inline]
    fn eq(&self, other: &Vec<'_, U, A>) -> bool {
        self[..] == other[..]
    }
}
impl<T: Eq, A: Allocator + Clone> Eq for Vec<'_, T, A> {}
impl<T: PartialOrd, A: Allocator + Clone> PartialOrd for Vec<'_, T, A> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.as_slice().partial_cmp(other.as_slice())
    }
}
impl<T: Ord, A: Allocator + Clone> Ord for Vec<'_, T, A> {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}
impl<T: Hash, A: Allocator + Clone> Hash for Vec<'_, T, A> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl<T: Clone, A: Allocator + Clone> Clone for Vec<'_, T, A> {
    fn clone(&self) -> Self {
        let mut out = Vec::with_capacity_in(self.len(), self.arena);
        for item in self.as_slice() {
            out.push(item.clone());
        }
        out
    }
}

impl<'a, T, A: Allocator + Clone> IntoIterator for Vec<'a, T, A> {
    type Item = T;
    type IntoIter = IntoIter<'a, T, A>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        IntoIter::new(self)
    }
}

impl<'b, T, A: Allocator + Clone> IntoIterator for &'b Vec<'_, T, A> {
    type Item = &'b T;
    type IntoIter = slice::Iter<'b, T>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

impl<'b, T, A: Allocator + Clone> IntoIterator for &'b mut Vec<'_, T, A> {
    type Item = &'b mut T;
    type IntoIter = slice::IterMut<'b, T>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.as_mut_slice().iter_mut()
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<T: serde::ser::Serialize, A: Allocator + Clone> serde::ser::Serialize for Vec<'_, T, A> {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_slice().serialize(serializer)
    }
}

impl<'a, T, A: Allocator + Clone> crate::vec::FromIteratorIn<'a, T, A> for Vec<'a, T, A> {
    #[inline]
    fn from_iter_in<I: IntoIterator<Item = T>>(iter: I, arena: &'a Arena<A>) -> Self {
        Self::from_iter_in(iter, arena)
    }
}

#[cfg(feature = "std")]
use std::io;

#[cfg(feature = "std")]
impl<A: Allocator + Clone> io::Write for Vec<'_, u8, A> {
    /// Appends `buf` to the vector. Always succeeds with `buf.len()`.
    /// Panics if the backing allocator fails (matching `std::vec::Vec`).
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.extend_from_slice(buf);
        Ok(buf.len())
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.extend_from_slice(buf);
        Ok(())
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<I: slice::SliceIndex<[T]>, T, A: Allocator + Clone> core::ops::Index<I> for Vec<'_, T, A> {
    type Output = I::Output;
    #[inline]
    fn index(&self, index: I) -> &Self::Output {
        core::ops::Index::index(&**self, index)
    }
}

impl<I: slice::SliceIndex<[T]>, T, A: Allocator + Clone> core::ops::IndexMut<I> for Vec<'_, T, A> {
    #[inline]
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        core::ops::IndexMut::index_mut(&mut **self, index)
    }
}

impl<T, A: Allocator + Clone> AsRef<Self> for Vec<'_, T, A> {
    #[inline]
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<T, A: Allocator + Clone> AsMut<Self> for Vec<'_, T, A> {
    #[inline]
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

impl<'a, T, A: Allocator + Clone, const N: usize> TryFrom<Vec<'a, T, A>> for [T; N] {
    type Error = Vec<'a, T, A>;

    /// Consume the `Vec` into an array `[T; N]` when `len == N`; on a length
    /// mismatch the original `Vec` is returned unchanged. Mirrors `std`'s
    /// `TryFrom<Vec<T>> for [T; N]`.
    fn try_from(mut v: Vec<'a, T, A>) -> Result<Self, Self::Error> {
        if v.len() != N {
            return Err(v);
        }
        // SAFETY: `v` has exactly `N` initialized elements. Read them out as
        // an array, then set the length to 0 so the `Vec`'s `Drop` does not
        // re-drop the moved-out elements (the backing buffer is released
        // without touching them).
        unsafe {
            let arr = core::ptr::read(v.as_ptr().cast::<[T; N]>());
            v.set_len(0);
            Ok(arr)
        }
    }
}
