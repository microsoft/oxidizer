// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-aware [`FromIn`] conversions for [`Vec`], mirroring `std`'s
//! `From<&[T]>` / `From<&mut [T]>` / `From<[T; N]>` / `From<Box<[T]>>` /
//! `From<Cow<[T]>>` impls.

use alloc::borrow::Cow;
use alloc::boxed::Box as StdBox;

use allocator_api2::alloc::Allocator;

use super::Vec;
use crate::{Arena, FromIn};

impl<'a, 'b, T: Clone, A: Allocator + Clone> FromIn<'a, &'b [T], A> for Vec<'a, T, A> {
    /// Clone each element of `value` into a fresh arena vector. Mirrors
    /// `std`'s `From<&[T]> for Vec<T>`.
    #[inline]
    fn from_in(value: &'b [T], arena: &'a Arena<A>) -> Self {
        Self::from_iter_in(value.iter().cloned(), arena)
    }
}

impl<'a, 'b, T: Clone, A: Allocator + Clone> FromIn<'a, &'b mut [T], A> for Vec<'a, T, A> {
    /// Clone each element of `value` into a fresh arena vector. Mirrors
    /// `std`'s `From<&mut [T]> for Vec<T>`.
    #[inline]
    fn from_in(value: &'b mut [T], arena: &'a Arena<A>) -> Self {
        Self::from_iter_in(value.iter().cloned(), arena)
    }
}

impl<'a, T, A: Allocator + Clone, const N: usize> FromIn<'a, [T; N], A> for Vec<'a, T, A> {
    /// Move the array's elements into a fresh arena vector. Mirrors `std`'s
    /// `From<[T; N]> for Vec<T>`.
    #[inline]
    fn from_in(value: [T; N], arena: &'a Arena<A>) -> Self {
        Self::from_iter_in(value, arena)
    }
}

impl<'a, T, A: Allocator + Clone> FromIn<'a, StdBox<[T]>, A> for Vec<'a, T, A> {
    /// Move the boxed slice's elements into a fresh arena vector. Mirrors
    /// `std`'s `From<Box<[T]>> for Vec<T>`.
    #[inline]
    fn from_in(value: StdBox<[T]>, arena: &'a Arena<A>) -> Self {
        Self::from_iter_in(value, arena)
    }
}

impl<'a, 'b, T: Clone, A: Allocator + Clone> FromIn<'a, Cow<'b, [T]>, A> for Vec<'a, T, A> {
    /// Build a fresh arena vector from a clone-on-write slice — cloning a
    /// borrowed slice or moving an owned one. Mirrors `std`'s
    /// `From<Cow<[T]>> for Vec<T>`.
    #[inline]
    fn from_in(value: Cow<'b, [T]>, arena: &'a Arena<A>) -> Self {
        match value {
            Cow::Borrowed(s) => Self::from_iter_in(s.iter().cloned(), arena),
            Cow::Owned(v) => Self::from_iter_in(v, arena),
        }
    }
}
