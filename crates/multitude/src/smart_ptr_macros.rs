// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared forwarding-trait macros for [`Rc`](crate::Rc) and
//! [`Arc`](crate::Arc): `Deref`, `Debug`, `Display`, `PartialEq`,
//! `Eq`, `PartialOrd`, `Ord`, `Hash`, `AsRef<T>`, `Borrow<T>`,
//! `Pointer`, `Unpin`. These don't depend on the refcount flavor.
//!
//! `Clone` and `Drop` differ between Rc/Arc and stay hand-written.

/// Emit the forwarding trait impls shared by [`Rc`](crate::Rc) and
/// [`Arc`](crate::Arc) (`Deref`, `Debug`, `Display`, `PartialEq`,
/// `Eq`, `PartialOrd`, `Ord`, `Hash`, `AsRef<T>`, `Borrow<T>`,
/// `Pointer`, `Unpin`).
///
/// `$Ty` is the smart-pointer type name (`Rc` or `Arc`).
macro_rules! impl_smart_ptr_forwarding_traits {
    ($Ty:ident) => {
        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> ::core::ops::Deref for $Ty<T, A> {
            type Target = T;
            #[inline]
            fn deref(&self) -> &T {
                // SAFETY: refcount-positive invariant — `self` holds a +1, so
                // the chunk and the value within it are live.
                unsafe { self.ptr.as_ref() }
            }
        }

        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> ::core::fmt::Debug for $Ty<T, A>
        where
            T: ::core::fmt::Debug,
        {
            #[inline]
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Debug::fmt(&**self, f)
            }
        }

        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> ::core::fmt::Display for $Ty<T, A>
        where
            T: ::core::fmt::Display,
        {
            #[inline]
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Display::fmt(&**self, f)
            }
        }

        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> ::core::cmp::PartialEq for $Ty<T, A>
        where
            T: ::core::cmp::PartialEq,
        {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                **self == **other
            }
        }

        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> ::core::cmp::Eq for $Ty<T, A> where T: ::core::cmp::Eq {}

        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> ::core::cmp::PartialOrd for $Ty<T, A>
        where
            T: ::core::cmp::PartialOrd,
        {
            #[inline]
            fn partial_cmp(&self, other: &Self) -> ::core::option::Option<::core::cmp::Ordering> {
                (**self).partial_cmp(&**other)
            }
        }

        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> ::core::cmp::Ord for $Ty<T, A>
        where
            T: ::core::cmp::Ord,
        {
            #[inline]
            fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
                (**self).cmp(&**other)
            }
        }

        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> ::core::hash::Hash for $Ty<T, A>
        where
            T: ::core::hash::Hash,
        {
            #[inline]
            fn hash<H: ::core::hash::Hasher>(&self, state: &mut H) {
                (**self).hash(state);
            }
        }

        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> ::core::convert::AsRef<T> for $Ty<T, A> {
            #[inline]
            fn as_ref(&self) -> &T {
                self
            }
        }

        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> ::core::borrow::Borrow<T> for $Ty<T, A> {
            #[inline]
            fn borrow(&self) -> &T {
                self
            }
        }

        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> ::core::fmt::Pointer for $Ty<T, A> {
            #[inline]
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Pointer::fmt(&self.ptr.as_ptr(), f)
            }
        }

        impl<T: ?Sized, A: ::allocator_api2::alloc::Allocator + Clone> Unpin for $Ty<T, A> {}
    };
}

pub(crate) use impl_smart_ptr_forwarding_traits;
