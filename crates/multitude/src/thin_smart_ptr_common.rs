// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared inherent methods and trait impls for the three thin
//! smart-pointer types ([`Arc`](crate::Arc), [`Rc`](crate::Rc), and
//! [`Box`](crate::Box)).
//!
//! All three share an identical layout (`NonNull<u8>` thin pointer +
//! `PhantomData<(*const T, A)>`), an identical metadata-recovery
//! helper ([`as_fat_ptr`](crate::internal::thin_dst::as_fat)), and
//! identical forwarding trait impls (`Deref`, `AsRef`, `Borrow`,
//! `Debug`, `Display`, ordering, hashing, `Pointer`, `Unpin`,
//! [`Pin`] conversion). The macro below emits all of that for a given
//! struct name; per-file blocks supply the items that legitimately
//! differ (`Send`/`Sync` bounds, `Drop`, `Clone` for `Arc`/`Rc`, mutable
//! accessors for `Box`, iterator forwarding for `Box`, etc.).

/// Emit shared inherent methods + read-only trait impls for a thin
/// smart pointer of layout `{ ptr: NonNull<u8>, _phantom }`.
macro_rules! impl_thin_smart_ptr_common {
    ($Ty:ident) => {
        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> $Ty<T, A> {
            /// Reconstructs the (possibly fat) `NonNull<T>` from the thin
            /// storage by reading `T`'s metadata from the chunk prefix.
            ///
            /// Zero-cost for `T: Sized` (metadata is `()`, no memory access).
            #[inline]
            fn as_fat_ptr(&self) -> core::ptr::NonNull<T> {
                // SAFETY: chunk allocator wrote `T::Metadata` at
                // `self.ptr - size_of::<T::Metadata>()`; the chunk is
                // kept alive by `self`'s ownership/refcount.
                unsafe { $crate::internal::thin_dst::as_fat::<T>(self.ptr) }
            }

            /// Returns a raw pointer to the value (fat if `T: ?Sized` is a DST).
            ///
            /// ```
            /// let arena = multitude::Arena::new();
            /// let arc = arena.alloc_arc(7_u32);
            /// let rc = arena.alloc_rc(8_u32);
            /// let boxed = arena.alloc_box(9_u32);
            /// assert_eq!(unsafe { *arc.as_ptr() }, 7);
            /// assert_eq!(unsafe { *rc.as_ptr() }, 8);
            /// assert_eq!(unsafe { *boxed.as_ptr() }, 9);
            /// ```
            #[inline]
            #[must_use]
            pub fn as_ptr(&self) -> *const T {
                self.as_fat_ptr().as_ptr().cast_const()
            }

            /// Convert into a [`Pin`](core::pin::Pin) of `Self`.
            ///
            /// Sound for any `T` (including `!Unpin`) because the
            /// value's address is fixed at allocation time, the
            /// containing smart pointer keeps the storage alive at the
            /// same address through `Drop`, and the value is dropped at
            /// the same address — satisfying `Pin`'s contract.
            ///
            /// ```
            /// use multitude::{Arc, Arena, Box, Rc};
            ///
            /// let arena = Arena::new();
            /// assert_eq!(*Arc::into_pin(arena.alloc_arc(1_u8)), 1);
            /// assert_eq!(*Rc::into_pin(arena.alloc_rc(2_u8)), 2);
            /// assert_eq!(*Box::into_pin(arena.alloc_box(3_u8)), 3);
            /// ```
            #[must_use]
            #[inline]
            pub fn into_pin(this: Self) -> core::pin::Pin<Self> {
                // SAFETY: the value's address is fixed at allocation time and the
                // smart pointer keeps the storage alive at that same address
                // through `Drop`, where the value is finalized — exactly `Pin`'s
                // contract, so pinning is sound for any `T` (including `!Unpin`).
                unsafe { core::pin::Pin::new_unchecked(this) }
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> From<$Ty<T, A>> for core::pin::Pin<$Ty<T, A>> {
            /// Mirror of `From<std::sync::Arc<T>> for Pin<std::sync::Arc<T>>` /
            /// `From<std::boxed::Box<T>> for Pin<std::boxed::Box<T>>`.
            #[inline]
            fn from(p: $Ty<T, A>) -> Self {
                $Ty::into_pin(p)
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> core::ops::Deref for $Ty<T, A> {
            type Target = T;
            #[inline]
            fn deref(&self) -> &T {
                // SAFETY: `ptr` references a live, initialized `T` that
                // is kept alive by `self` (refcount for `Arc`, exclusive
                // ownership for `Box`).
                unsafe { self.as_fat_ptr().as_ref() }
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> AsRef<T> for $Ty<T, A> {
            #[inline]
            fn as_ref(&self) -> &T {
                self
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> core::borrow::Borrow<T> for $Ty<T, A> {
            #[inline]
            fn borrow(&self) -> &T {
                self
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> core::fmt::Debug for $Ty<T, A>
        where
            T: core::fmt::Debug,
        {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Debug::fmt(&**self, f)
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> core::fmt::Display for $Ty<T, A>
        where
            T: core::fmt::Display,
        {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Display::fmt(&**self, f)
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> PartialEq for $Ty<T, A>
        where
            T: PartialEq,
        {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                PartialEq::eq(&**self, &**other)
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> Eq for $Ty<T, A> where T: Eq {}

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> PartialOrd for $Ty<T, A>
        where
            T: PartialOrd,
        {
            #[inline]
            fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
                PartialOrd::partial_cmp(&**self, &**other)
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> Ord for $Ty<T, A>
        where
            T: Ord,
        {
            #[inline]
            fn cmp(&self, other: &Self) -> core::cmp::Ordering {
                Ord::cmp(&**self, &**other)
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> core::hash::Hash for $Ty<T, A>
        where
            T: core::hash::Hash,
        {
            #[inline]
            fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
                (**self).hash(state);
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> core::fmt::Pointer for $Ty<T, A> {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Pointer::fmt(&self.ptr.as_ptr(), f)
            }
        }

        impl<T: ?Sized + ptr_meta::Pointee, A: allocator_api2::alloc::Allocator + Clone> Unpin for $Ty<T, A> {}
    };
}

pub(crate) use impl_thin_smart_ptr_common;
