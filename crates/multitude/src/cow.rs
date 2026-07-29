// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::borrow::Borrow;
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::ops::Deref;

use allocator_api2::alloc::{Allocator, Global};
use ptr_meta::Pointee;

use crate::{AllocError, Arena, Box};

/// A value that is either borrowed or owned by an arena-backed [`Box`].
///
/// Unlike [`alloc::borrow::Cow`], cloning or converting a borrowed value to
/// owned storage requires an [`Arena`]. Use [`Cow::clone_in`],
/// [`Cow::into_owned`], or [`Cow::to_mut`] to supply that arena explicitly.
///
/// ```
/// use multitude::{Arena, Cow};
///
/// let arena = Arena::new();
/// let mut value: Cow<'_, str> = Cow::Borrowed("hello");
/// value.to_mut(&arena).make_ascii_uppercase();
/// assert_eq!(&*value, "HELLO");
/// assert!(value.is_owned());
/// ```
pub enum Cow<'a, T: ?Sized + Pointee, A: Allocator + Clone = Global> {
    /// A value borrowed from existing storage.
    Borrowed(&'a T),
    /// A value owned by an escape-capable arena smart pointer.
    Owned(Box<T, A>),
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Cow<'_, T, A> {
    /// Returns `true` when the value is borrowed.
    #[must_use]
    pub const fn is_borrowed(&self) -> bool {
        matches!(self, Self::Borrowed(_))
    }

    /// Returns `true` when the value is arena-owned.
    #[must_use]
    pub const fn is_owned(&self) -> bool {
        matches!(self, Self::Owned(_))
    }
}

impl<T: Clone, A: Allocator + Clone> Cow<'_, T, A> {
    /// Converts this value to arena-owned storage.
    ///
    /// An owned value is returned unchanged, even if it originated in a
    /// different arena.
    #[must_use]
    pub fn into_owned(self, arena: &Arena<A>) -> Box<T, A> {
        match self {
            Self::Borrowed(value) => arena.alloc_box(value.clone()),
            Self::Owned(value) => value,
        }
    }

    /// Converts this value to arena-owned storage.
    ///
    /// An owned value is returned unchanged, even if it originated in a
    /// different arena.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if a borrowed value cannot be copied into
    /// `arena`.
    pub fn try_into_owned(self, arena: &Arena<A>) -> Result<Box<T, A>, AllocError> {
        match self {
            Self::Borrowed(value) => arena.try_alloc_box(value.clone()),
            Self::Owned(value) => Ok(value),
        }
    }

    /// Returns mutable access, copying a borrowed value into `arena` first.
    pub fn to_mut<'b>(&'b mut self, arena: &Arena<A>) -> &'b mut T {
        match self {
            Self::Borrowed(value) => {
                let owned = arena.alloc_box((*value).clone());
                *self = Self::Owned(owned);
                self.to_mut(arena)
            }
            Self::Owned(value) => value,
        }
    }

    /// Returns mutable access, copying a borrowed value into `arena` first.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if a borrowed value cannot be copied into
    /// `arena`.
    pub fn try_to_mut<'b>(&'b mut self, arena: &Arena<A>) -> Result<&'b mut T, AllocError> {
        match self {
            Self::Borrowed(value) => {
                let owned = arena.try_alloc_box((*value).clone())?;
                *self = Self::Owned(owned);
                self.try_to_mut(arena)
            }
            Self::Owned(value) => Ok(value),
        }
    }

    /// Clones this value, using `arena` for owned storage.
    #[must_use]
    pub fn clone_in(&self, arena: &Arena<A>) -> Self {
        match self {
            Self::Borrowed(value) => Self::Borrowed(value),
            Self::Owned(value) => Self::Owned(arena.alloc_box((**value).clone())),
        }
    }

    /// Clones this value, using `arena` for owned storage.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if an owned value cannot be copied into `arena`.
    pub fn try_clone_in(&self, arena: &Arena<A>) -> Result<Self, AllocError> {
        match self {
            Self::Borrowed(value) => Ok(Self::Borrowed(value)),
            Self::Owned(value) => arena.try_alloc_box((**value).clone()).map(Self::Owned),
        }
    }
}

impl<A: Allocator + Clone> Cow<'_, str, A> {
    /// Converts this string to arena-owned storage.
    ///
    /// An owned string is returned unchanged, even if it originated in a
    /// different arena.
    #[must_use]
    pub fn into_owned(self, arena: &Arena<A>) -> Box<str, A> {
        match self {
            Self::Borrowed(value) => arena.alloc_str_box(value),
            Self::Owned(value) => value,
        }
    }

    /// Converts this string to arena-owned storage.
    ///
    /// An owned string is returned unchanged, even if it originated in a
    /// different arena.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if a borrowed string cannot be copied into
    /// `arena`.
    pub fn try_into_owned(self, arena: &Arena<A>) -> Result<Box<str, A>, AllocError> {
        match self {
            Self::Borrowed(value) => arena.try_alloc_str_box(value),
            Self::Owned(value) => Ok(value),
        }
    }

    /// Returns mutable access, copying a borrowed string into `arena` first.
    pub fn to_mut<'b>(&'b mut self, arena: &Arena<A>) -> &'b mut str {
        match self {
            Self::Borrowed(value) => {
                let owned = arena.alloc_str_box(*value);
                *self = Self::Owned(owned);
                self.to_mut(arena)
            }
            Self::Owned(value) => value,
        }
    }

    /// Returns mutable access, copying a borrowed string into `arena` first.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if a borrowed string cannot be copied into
    /// `arena`.
    pub fn try_to_mut<'b>(&'b mut self, arena: &Arena<A>) -> Result<&'b mut str, AllocError> {
        match self {
            Self::Borrowed(value) => {
                let owned = arena.try_alloc_str_box(*value)?;
                *self = Self::Owned(owned);
                self.try_to_mut(arena)
            }
            Self::Owned(value) => Ok(value),
        }
    }

    /// Clones this string, using `arena` for owned storage.
    #[must_use]
    pub fn clone_in(&self, arena: &Arena<A>) -> Self {
        match self {
            Self::Borrowed(value) => Self::Borrowed(value),
            Self::Owned(value) => Self::Owned(arena.alloc_str_box(value)),
        }
    }

    /// Clones this string, using `arena` for owned storage.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if an owned string cannot be copied into `arena`.
    pub fn try_clone_in(&self, arena: &Arena<A>) -> Result<Self, AllocError> {
        match self {
            Self::Borrowed(value) => Ok(Self::Borrowed(value)),
            Self::Owned(value) => arena.try_alloc_str_box(value).map(Self::Owned),
        }
    }
}

impl<T: Clone, A: Allocator + Clone> Cow<'_, [T], A> {
    /// Converts this slice to arena-owned storage.
    ///
    /// An owned slice is returned unchanged, even if it originated in a
    /// different arena.
    #[must_use]
    pub fn into_owned(self, arena: &Arena<A>) -> Box<[T], A> {
        match self {
            Self::Borrowed(value) => arena.alloc_slice_clone_box(value),
            Self::Owned(value) => value,
        }
    }

    /// Converts this slice to arena-owned storage.
    ///
    /// An owned slice is returned unchanged, even if it originated in a
    /// different arena.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if a borrowed slice cannot be copied into
    /// `arena`.
    pub fn try_into_owned(self, arena: &Arena<A>) -> Result<Box<[T], A>, AllocError> {
        match self {
            Self::Borrowed(value) => arena.try_alloc_slice_clone_box(value),
            Self::Owned(value) => Ok(value),
        }
    }

    /// Returns mutable access, copying a borrowed slice into `arena` first.
    pub fn to_mut<'b>(&'b mut self, arena: &Arena<A>) -> &'b mut [T] {
        match self {
            Self::Borrowed(value) => {
                let owned = arena.alloc_slice_clone_box(*value);
                *self = Self::Owned(owned);
                self.to_mut(arena)
            }
            Self::Owned(value) => value,
        }
    }

    /// Returns mutable access, copying a borrowed slice into `arena` first.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if a borrowed slice cannot be copied into
    /// `arena`.
    pub fn try_to_mut<'b>(&'b mut self, arena: &Arena<A>) -> Result<&'b mut [T], AllocError> {
        match self {
            Self::Borrowed(value) => {
                let owned = arena.try_alloc_slice_clone_box(*value)?;
                *self = Self::Owned(owned);
                self.try_to_mut(arena)
            }
            Self::Owned(value) => Ok(value),
        }
    }

    /// Clones this slice, using `arena` for owned storage.
    #[must_use]
    pub fn clone_in(&self, arena: &Arena<A>) -> Self {
        match self {
            Self::Borrowed(value) => Self::Borrowed(value),
            Self::Owned(value) => Self::Owned(arena.alloc_slice_clone_box(value)),
        }
    }

    /// Clones this slice, using `arena` for owned storage.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if an owned slice cannot be copied into `arena`.
    pub fn try_clone_in(&self, arena: &Arena<A>) -> Result<Self, AllocError> {
        match self {
            Self::Borrowed(value) => Ok(Self::Borrowed(value)),
            Self::Owned(value) => arena.try_alloc_slice_clone_box(value).map(Self::Owned),
        }
    }
}

impl<'a, T: ?Sized + Pointee, A: Allocator + Clone> From<&'a T> for Cow<'a, T, A> {
    fn from(value: &'a T) -> Self {
        Self::Borrowed(value)
    }
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> From<Box<T, A>> for Cow<'_, T, A> {
    fn from(value: Box<T, A>) -> Self {
        Self::Owned(value)
    }
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Deref for Cow<'_, T, A> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(value) => value,
            Self::Owned(value) => value,
        }
    }
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> AsRef<T> for Cow<'_, T, A> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized + Pointee, A: Allocator + Clone> Borrow<T> for Cow<'_, T, A> {
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized + Pointee + fmt::Debug, A: Allocator + Clone> fmt::Debug for Cow<'_, T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + Pointee + fmt::Display, A: Allocator + Clone> fmt::Display for Cow<'_, T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: ?Sized + Pointee + PartialEq, A: Allocator + Clone> PartialEq for Cow<'_, T, A> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: ?Sized + Pointee + Eq, A: Allocator + Clone> Eq for Cow<'_, T, A> {}

impl<T: ?Sized + Pointee + PartialOrd, A: Allocator + Clone> PartialOrd for Cow<'_, T, A> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }
}

impl<T: ?Sized + Pointee + Ord, A: Allocator + Clone> Ord for Cow<'_, T, A> {
    fn cmp(&self, other: &Self) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<T: ?Sized + Pointee + Hash, A: Allocator + Clone> Hash for Cow<'_, T, A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}

#[cfg(feature = "serde")]
impl<T: ?Sized + Pointee + serde::Serialize, A: Allocator + Clone> serde::Serialize for Cow<'_, T, A> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (**self).serialize(serializer)
    }
}
