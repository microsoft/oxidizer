// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared inherent methods and forwarding trait impls for the four thin
//! handle types ([`Box`](crate::Box), [`Alloc`](crate::Alloc),
//! [`Arc`](crate::Arc), [`Rc`](crate::Rc)).
//!
//! All four use a single pointer for sized values and forward `Deref`-able
//! operations to the contained value, so the macros below emit the read-only
//! surface once. `Alloc` points to `SlotCell<T>` while `Box`/`Arc`/`Rc` point
//! directly to `T` and may therefore carry DST metadata. Per-file blocks keep
//! what legitimately differs (`Send`/`Sync`, `Drop`, `Clone` for `Arc`/`Rc`,
//! and mutable access for `Box`/`Alloc`). The optional `$lt` lifetime lets the
//! same macro serve `Alloc<'pool, T, A>`.

/// Emits the shared inherent methods and read-only forwarding trait impls for a
/// thin handle whose value is reached through `Deref`.
macro_rules! impl_handle_common {
    ($Ty:ident $(, $lt:lifetime)?) => {
        impl<$($lt,)? T, A: allocator_api2::alloc::Allocator> $Ty<$($lt,)? T, A> {
            /// Returns a raw pointer to the value in its pool slot.
            ///
            /// The value never moves while any handle to it is alive, so the
            /// pointer is stable. It is valid for reads as long as a handle
            /// keeps the slot occupied.
            #[must_use]
            #[inline]
            pub fn as_ptr(this: &Self) -> *const T {
                core::ptr::from_ref::<T>(&**this)
            }

            /// Converts the handle into a [`Pin`](core::pin::Pin) of itself.
            ///
            /// Sound for any `T` (including `!Unpin`): the value's address is
            /// fixed at allocation and the handle keeps the slot alive at that
            /// same address until the value is dropped — exactly `Pin`'s
            /// contract.
            #[must_use]
            #[inline]
            pub fn into_pin(this: Self) -> core::pin::Pin<Self> {
                // SAFETY: the value's address is fixed for the handle's lifetime
                // (the pool never moves an occupied slot), satisfying `Pin` even
                // for `!Unpin` `T`. `Pin::new` is unusable here: it requires the
                // pointee `T: Unpin`, but this must accept `!Unpin` values.
                unsafe { core::pin::Pin::new_unchecked(this) }
            }
        }

        impl<$($lt,)? T, A: allocator_api2::alloc::Allocator> From<$Ty<$($lt,)? T, A>>
            for core::pin::Pin<$Ty<$($lt,)? T, A>>
        {
            #[inline]
            fn from(handle: $Ty<$($lt,)? T, A>) -> Self {
                $Ty::into_pin(handle)
            }
        }

        // The handle's own address is irrelevant to the pinned value (which
        // lives in the pool slot), so the handle is always `Unpin`, mirroring
        // `std`'s smart pointers.
        impl<$($lt,)? T, A: allocator_api2::alloc::Allocator> Unpin for $Ty<$($lt,)? T, A> {}

        impl<$($lt,)? T, A: allocator_api2::alloc::Allocator> AsRef<T> for $Ty<$($lt,)? T, A> {
            #[inline]
            fn as_ref(&self) -> &T {
                &**self
            }
        }

        impl<$($lt,)? T, A: allocator_api2::alloc::Allocator> core::borrow::Borrow<T>
            for $Ty<$($lt,)? T, A>
        {
            #[inline]
            fn borrow(&self) -> &T {
                &**self
            }
        }

        impl<$($lt,)? T: core::fmt::Debug, A: allocator_api2::alloc::Allocator> core::fmt::Debug
            for $Ty<$($lt,)? T, A>
        {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Debug::fmt(&**self, f)
            }
        }

        impl<$($lt,)? T: core::fmt::Display, A: allocator_api2::alloc::Allocator> core::fmt::Display
            for $Ty<$($lt,)? T, A>
        {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Display::fmt(&**self, f)
            }
        }

        impl<$($lt,)? T: PartialEq, A: allocator_api2::alloc::Allocator> PartialEq
            for $Ty<$($lt,)? T, A>
        {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                PartialEq::eq(&**self, &**other)
            }
        }

        impl<$($lt,)? T: Eq, A: allocator_api2::alloc::Allocator> Eq for $Ty<$($lt,)? T, A> {}

        impl<$($lt,)? T: PartialOrd, A: allocator_api2::alloc::Allocator> PartialOrd
            for $Ty<$($lt,)? T, A>
        {
            #[inline]
            fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
                PartialOrd::partial_cmp(&**self, &**other)
            }
        }

        impl<$($lt,)? T: Ord, A: allocator_api2::alloc::Allocator> Ord for $Ty<$($lt,)? T, A> {
            #[inline]
            fn cmp(&self, other: &Self) -> core::cmp::Ordering {
                Ord::cmp(&**self, &**other)
            }
        }

        impl<$($lt,)? T: core::hash::Hash, A: allocator_api2::alloc::Allocator> core::hash::Hash
            for $Ty<$($lt,)? T, A>
        {
            #[inline]
            fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
                (**self).hash(state);
            }
        }

        impl<$($lt,)? T, A: allocator_api2::alloc::Allocator> core::fmt::Pointer
            for $Ty<$($lt,)? T, A>
        {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Pointer::fmt(&self.slot.as_ptr(), f)
            }
        }
    };
}

/// Emits the mutable accessors for the unique-owner handles (`Box`, `Alloc`),
/// which additionally implement `DerefMut`.
macro_rules! impl_handle_mut {
    ($Ty:ident $(, $lt:lifetime)?) => {
        impl<$($lt,)? T, A: allocator_api2::alloc::Allocator> $Ty<$($lt,)? T, A> {
            /// Returns a raw mutable pointer to the value in its pool slot.
            ///
            /// The value never moves while this handle is alive, so the pointer
            /// is stable.
            #[must_use]
            #[inline]
            pub fn as_mut_ptr(this: &mut Self) -> *mut T {
                core::ptr::from_mut::<T>(&mut **this)
            }
        }

        impl<$($lt,)? T, A: allocator_api2::alloc::Allocator> AsMut<T> for $Ty<$($lt,)? T, A> {
            #[inline]
            fn as_mut(&mut self) -> &mut T {
                &mut **self
            }
        }

        impl<$($lt,)? T, A: allocator_api2::alloc::Allocator> core::borrow::BorrowMut<T>
            for $Ty<$($lt,)? T, A>
        {
            #[inline]
            fn borrow_mut(&mut self) -> &mut T {
                &mut **self
            }
        }
    };
}

pub(crate) use impl_handle_common;
pub(crate) use impl_handle_mut;

/// Like [`impl_handle_common`] but for the `T: ?Sized` handles ([`Box`](crate::Box),
/// [`Arc`](crate::Arc), [`Rc`](crate::Rc)) whose `slot` field is a `NonNull<T>`
/// (a fat pointer for unsized `T`). Emits the same read-only forwarding surface
/// with `T: ?Sized` bounds.
macro_rules! impl_handle_common_unsized {
    ($Ty:ident) => {
        impl<T: ?Sized, A: allocator_api2::alloc::Allocator> $Ty<T, A> {
            /// Returns a raw pointer to the value in its pool slot.
            ///
            /// The value never moves while any handle to it is alive, so the
            /// pointer is stable. It is valid for reads as long as a handle keeps
            /// the slot occupied.
            #[must_use]
            #[inline]
            pub fn as_ptr(this: &Self) -> *const T {
                core::ptr::from_ref::<T>(&**this)
            }

            /// Converts the handle into a [`Pin`](core::pin::Pin) of itself.
            ///
            /// Sound for any `T` (including `!Unpin`): the value's address is
            /// fixed at allocation and the handle keeps the slot alive at that
            /// same address until the value is dropped.
            #[must_use]
            #[inline]
            pub fn into_pin(this: Self) -> core::pin::Pin<Self> {
                // SAFETY: the value's address is fixed for the handle's lifetime
                // (the pool never moves an occupied slot), satisfying `Pin` even
                // for `!Unpin` `T`.
                unsafe { core::pin::Pin::new_unchecked(this) }
            }
        }

        impl<T: ?Sized, A: allocator_api2::alloc::Allocator> From<$Ty<T, A>> for core::pin::Pin<$Ty<T, A>> {
            #[inline]
            fn from(handle: $Ty<T, A>) -> Self {
                $Ty::into_pin(handle)
            }
        }

        // The handle's own address is irrelevant to the pinned value (which
        // lives in the pool slot), so the handle is always `Unpin`.
        impl<T: ?Sized, A: allocator_api2::alloc::Allocator> Unpin for $Ty<T, A> {}

        impl<T: ?Sized, A: allocator_api2::alloc::Allocator> AsRef<T> for $Ty<T, A> {
            #[inline]
            fn as_ref(&self) -> &T {
                self
            }
        }

        impl<T: ?Sized, A: allocator_api2::alloc::Allocator> core::borrow::Borrow<T> for $Ty<T, A> {
            #[inline]
            fn borrow(&self) -> &T {
                self
            }
        }

        impl<T: ?Sized + core::fmt::Debug, A: allocator_api2::alloc::Allocator> core::fmt::Debug for $Ty<T, A> {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Debug::fmt(&**self, f)
            }
        }

        impl<T: ?Sized + core::fmt::Display, A: allocator_api2::alloc::Allocator> core::fmt::Display for $Ty<T, A> {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Display::fmt(&**self, f)
            }
        }

        impl<T: ?Sized, A: allocator_api2::alloc::Allocator> core::fmt::Pointer for $Ty<T, A> {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Pointer::fmt(&self.slot.as_ptr(), f)
            }
        }

        impl<T: ?Sized + PartialEq, A: allocator_api2::alloc::Allocator> PartialEq for $Ty<T, A> {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                PartialEq::eq(&**self, &**other)
            }
        }

        impl<T: ?Sized + Eq, A: allocator_api2::alloc::Allocator> Eq for $Ty<T, A> {}

        impl<T: ?Sized + PartialOrd, A: allocator_api2::alloc::Allocator> PartialOrd for $Ty<T, A> {
            #[inline]
            fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
                PartialOrd::partial_cmp(&**self, &**other)
            }
        }

        impl<T: ?Sized + Ord, A: allocator_api2::alloc::Allocator> Ord for $Ty<T, A> {
            #[inline]
            fn cmp(&self, other: &Self) -> core::cmp::Ordering {
                Ord::cmp(&**self, &**other)
            }
        }

        impl<T: ?Sized + core::hash::Hash, A: allocator_api2::alloc::Allocator> core::hash::Hash for $Ty<T, A> {
            #[inline]
            fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
                (**self).hash(state);
            }
        }
    };
}

/// Like [`impl_handle_mut`] but for a `T: ?Sized` unique-owner handle
/// ([`Box`](crate::Box)): emits the mutable accessors with `T: ?Sized` bounds.
macro_rules! impl_handle_mut_unsized {
    ($Ty:ident) => {
        impl<T: ?Sized + Unpin, A: allocator_api2::alloc::Allocator> $Ty<T, A> {
            /// Returns a raw mutable pointer to the value in its pool slot.
            ///
            /// The value never moves while this handle is alive, so the pointer
            /// is stable.
            #[must_use]
            #[inline]
            pub fn as_mut_ptr(this: &mut Self) -> *mut T {
                core::ptr::from_mut::<T>(&mut **this)
            }
        }

        impl<T: ?Sized + Unpin, A: allocator_api2::alloc::Allocator> AsMut<T> for $Ty<T, A> {
            #[inline]
            fn as_mut(&mut self) -> &mut T {
                self
            }
        }

        impl<T: ?Sized + Unpin, A: allocator_api2::alloc::Allocator> core::borrow::BorrowMut<T> for $Ty<T, A> {
            #[inline]
            fn borrow_mut(&mut self) -> &mut T {
                self
            }
        }
    };
}

pub(crate) use impl_handle_common_unsized;
pub(crate) use impl_handle_mut_unsized;
