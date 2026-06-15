// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-aware analogs of [`From`] / [`Into`].
//!
//! Several `std` `From` impls (e.g. `From<&[T]> for Vec<T>`,
//! `From<&str> for String`) cannot be mirrored directly because they allocate
//! "from nothing" and thus require an allocator. [`FromIn`] is the arena-aware
//! counterpart: it threads the arena (`&'a Arena<A>`) through, exactly as
//! [`FromIteratorIn`](crate::vec::FromIteratorIn) does for
//! [`FromIterator`](core::iter::FromIterator).
//!
//! [`IntoIn`] is the [`Into`]-style companion, blanket-implemented for every
//! type convertible via [`FromIn`].
//!
//! ```
//! use multitude::vec::Vec;
//! use multitude::{Arena, FromIn, IntoIn};
//!
//! let arena = Arena::new();
//!
//! // Via `FromIn`:
//! let v: Vec<u32> = Vec::from_in([1_u32, 2, 3], &arena);
//! assert_eq!(&*v, &[1, 2, 3]);
//!
//! // Via `IntoIn` (the target type drives inference):
//! let w: Vec<u32> = [4_u32, 5].into_in(&arena);
//! assert_eq!(&*w, &[4, 5]);
//! ```

use allocator_api2::alloc::{Allocator, Global};

use crate::Arena;

/// Arena-aware counterpart to [`From`].
///
/// Builds `Self` from `value`, allocating into `arena`. Use this for
/// conversions that `std` exposes as [`From`] but that need an arena to
/// materialize the result.
pub trait FromIn<'a, T, A: Allocator + Clone = Global>: Sized {
    /// Builds `Self` from `value`, allocating into `arena`.
    fn from_in(value: T, arena: &'a Arena<A>) -> Self;
}

/// Arena-aware counterpart to [`Into`].
///
/// Provides `.into_in(arena)` for any `T` convertible into `C` via [`FromIn`],
/// through the conditional blanket impl below; the target collection `C` is
/// usually pinned by a type annotation. As with [`std::convert::Into`], you may
/// also implement `IntoIn` directly when the corresponding [`FromIn`] impl is
/// blocked by the orphan rule (e.g. converting a local type into a foreign
/// one), since the blanket only applies when `C: FromIn<Self>`.
///
/// ```
/// use multitude::strings::String;
/// use multitude::{Arena, IntoIn};
///
/// let arena = Arena::new();
/// let s: String = "hello".into_in(&arena);
/// assert_eq!(s.as_str(), "hello");
/// ```
pub trait IntoIn<'a, C, A: Allocator + Clone = Global>: Sized {
    /// Converts `self` into `C`, allocating into `arena`.
    fn into_in(self, arena: &'a Arena<A>) -> C;
}

// Mirrors `impl<T, U: From<T>> Into<U> for T`: the blanket is conditional on
// `C: FromIn<'a, Self, A>`, so a `(Self, C)` pair whose `FromIn` is blocked by
// the orphan rule may still hand-implement `IntoIn` without colliding here.
impl<'a, T, C, A: Allocator + Clone> IntoIn<'a, C, A> for T
where
    C: FromIn<'a, T, A>,
{
    #[inline]
    fn into_in(self, arena: &'a Arena<A>) -> C {
        C::from_in(self, arena)
    }
}
