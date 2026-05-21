// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Standard trait impls for [`Vec`].

use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

use allocator_api2::alloc::Allocator;
use allocator_api2::vec::Vec as ApiVec;

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
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, A: Allocator + Clone> AsMut<[T]> for Vec<'_, T, A> {
    fn as_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, A: Allocator + Clone> core::borrow::Borrow<[T]> for Vec<'_, T, A> {
    fn borrow(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, A: Allocator + Clone> core::borrow::BorrowMut<[T]> for Vec<'_, T, A> {
    fn borrow_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, A: Allocator + Clone> Extend<T> for Vec<'_, T, A> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        self.reserve(lower);
        for value in iter {
            self.push(value);
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
        for value in iter {
            self.push(*value);
        }
    }
}

impl<T: fmt::Debug, A: Allocator + Clone> fmt::Debug for Vec<'_, T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_slice().fmt(f)
    }
}

impl<T: PartialEq, A: Allocator + Clone> PartialEq for Vec<'_, T, A> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone> PartialEq<[U]> for Vec<'_, T, A> {
    fn eq(&self, other: &[U]) -> bool {
        self.as_slice() == other
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone> PartialEq<&[U]> for Vec<'_, T, A> {
    fn eq(&self, other: &&[U]) -> bool {
        self.as_slice() == *other
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone, const N: usize> PartialEq<[U; N]> for Vec<'_, T, A> {
    fn eq(&self, other: &[U; N]) -> bool {
        self.as_slice() == other.as_slice()
    }
}
impl<T: PartialEq<U>, U, A: Allocator + Clone, const N: usize> PartialEq<&[U; N]> for Vec<'_, T, A> {
    fn eq(&self, other: &&[U; N]) -> bool {
        self.as_slice() == other.as_slice()
    }
}
impl<T: Eq, A: Allocator + Clone> Eq for Vec<'_, T, A> {}
impl<T: PartialOrd, A: Allocator + Clone> PartialOrd for Vec<'_, T, A> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.as_slice().partial_cmp(other.as_slice())
    }
}
impl<T: Ord, A: Allocator + Clone> Ord for Vec<'_, T, A> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}
impl<T: Hash, A: Allocator + Clone> Hash for Vec<'_, T, A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl<T: Clone, A: Allocator + Clone> Clone for Vec<'_, T, A> {
    fn clone(&self) -> Self {
        let mut vec = Self::with_capacity_in(self.len, self.arena);
        vec.extend(self.as_slice().iter().cloned());
        vec
    }
}

impl<'a, T, A: Allocator + Clone> IntoIterator for Vec<'a, T, A> {
    type Item = T;
    type IntoIter = IntoIter<'a, T, A>;
    fn into_iter(self) -> Self::IntoIter {
        let me = ManuallyDrop::new(self);
        // SAFETY: these raw parts belong to `me` and were allocated through `me.arena`; ownership moves to `ApiVec`.
        unsafe { ApiVec::from_raw_parts_in(me.data.as_ptr(), me.len, me.cap, me.arena) }.into_iter()
    }
}

impl<'b, T, A: Allocator + Clone> IntoIterator for &'b Vec<'_, T, A> {
    type Item = &'b T;
    type IntoIter = core::slice::Iter<'b, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'b, T, A: Allocator + Clone> IntoIterator for &'b mut Vec<'_, T, A> {
    type Item = &'b mut T;
    type IntoIter = core::slice::IterMut<'b, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
impl<T: serde::ser::Serialize, A: Allocator + Clone> serde::ser::Serialize for Vec<'_, T, A> {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_slice().serialize(serializer)
    }
}

impl<'a, T, A: Allocator + Clone> crate::vec::FromIteratorIn<T> for Vec<'a, T, A> {
    type Allocator = &'a Arena<A>;

    fn from_iter_in<I: IntoIterator<Item = T>>(iter: I, allocator: &'a Arena<A>) -> Self {
        Vec::from_iter_in(iter, allocator)
    }
}

#[cfg(feature = "std")]
impl<A: Allocator + Clone> std::io::Write for Vec<'_, u8, A> {
    /// Appends `buf` to the vector. Always succeeds with `buf.len()`.
    /// Panics if the backing allocator fails (matching `std::vec::Vec`).
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.extend_from_slice(buf);
        Ok(buf.len())
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.extend_from_slice(buf);
        Ok(())
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
