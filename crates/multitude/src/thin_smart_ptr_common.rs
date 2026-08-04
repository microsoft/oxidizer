// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared inherent methods and trait impls for the three thin
//! smart-pointer types ([`Arc`](crate::Arc), [`Rc`](crate::Rc), and
//! [`Box`](crate::Box)).
//!
//! All three share a payload pointer, conditional per-handle metadata, a common
//! metadata-recovery helper ([`as_fat_ptr`](crate::internal::thin_dst::as_fat)),
//! and identical forwarding trait impls (`Deref`, `AsRef`, `Borrow`,
//! `Debug`, `Display`, ordering, hashing, `Pointer`, and `Unpin`).
//! The macro below emits all of that for a given
//! struct name; per-file blocks supply the items that legitimately
//! differ (`Send`/`Sync` bounds, `Drop`, `Clone` for `Arc`/`Rc`, mutable
//! accessors and pin conversion for `Box`, iterator forwarding for `Box`,
//! etc.).

/// Emit shared inherent methods and read-only trait impls.
macro_rules! impl_thin_smart_ptr_common {
    ($Ty:ident) => {
        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>>
            $Ty<T, A, M>
        {
            /// Reconstructs the (possibly fat) `NonNull<T>` from allocation-
            /// resident and handle-resident metadata.
            ///
            /// Zero-cost for `T: Sized` (metadata is `()`, no memory access).
            #[inline]
            fn as_fat_ptr(&self) -> core::ptr::NonNull<T> {
                // SAFETY: the allocation and this handle jointly retain the
                // metadata for the live value, and the handle keeps its chunk
                // alive.
                unsafe { $crate::internal::thin_dst::as_fat::<T, M>(self.ptr, self.metadata) }
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
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>>
            core::ops::Deref for $Ty<T, A, M>
        {
            type Target = T;
            #[inline]
            fn deref(&self) -> &T {
                // SAFETY: `ptr` references a live, initialized `T` that
                // is kept alive by `self` (refcount for `Arc`, exclusive
                // ownership for `Box`).
                unsafe { self.as_fat_ptr().as_ref() }
            }
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>> AsRef<T>
            for $Ty<T, A, M>
        {
            #[inline]
            fn as_ref(&self) -> &T {
                self
            }
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>>
            core::borrow::Borrow<T> for $Ty<T, A, M>
        {
            #[inline]
            fn borrow(&self) -> &T {
                self
            }
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>>
            core::fmt::Debug for $Ty<T, A, M>
        where
            T: core::fmt::Debug,
        {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Debug::fmt(&**self, f)
            }
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>>
            core::fmt::Display for $Ty<T, A, M>
        where
            T: core::fmt::Display,
        {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Display::fmt(&**self, f)
            }
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>> PartialEq
            for $Ty<T, A, M>
        where
            T: PartialEq,
        {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                PartialEq::eq(&**self, &**other)
            }
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>> Eq
            for $Ty<T, A, M>
        where
            T: Eq,
        {
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>> PartialOrd
            for $Ty<T, A, M>
        where
            T: PartialOrd,
        {
            #[inline]
            fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
                PartialOrd::partial_cmp(&**self, &**other)
            }
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>> Ord
            for $Ty<T, A, M>
        where
            T: Ord,
        {
            #[inline]
            fn cmp(&self, other: &Self) -> core::cmp::Ordering {
                Ord::cmp(&**self, &**other)
            }
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>>
            core::hash::Hash for $Ty<T, A, M>
        where
            T: core::hash::Hash,
        {
            #[inline]
            fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
                (**self).hash(state);
            }
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>>
            core::fmt::Pointer for $Ty<T, A, M>
        {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Pointer::fmt(&self.ptr.as_ptr(), f)
            }
        }

        impl<T: ?Sized + $crate::SmartPointerPointee, A: allocator_api2::alloc::Allocator + Clone, M: $crate::HandleMetadata<T>> Unpin
            for $Ty<T, A, M>
        {
        }
    };
}

pub(crate) use impl_thin_smart_ptr_common;
