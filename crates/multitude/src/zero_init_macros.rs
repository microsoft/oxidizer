// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared macro generating the body of the `bytemuck` / `zerocopy`
//! integration modules.
//!
//! The two ecosystems expose nearly identical zero-init traits
//! ([`bytemuck::Zeroable`] and [`zerocopy::FromZeros`]); the views
//! that expose safe zero-initialized arena allocations to user code
//! have identical method bodies and only differ in the trait bound.
//! Rather than maintain two ~290-line copies that drift over time,
//! both `bytemuck.rs` and `zerocopy.rs` invoke this macro with their
//! ecosystem-specific names. Method bodies live in exactly one
//! place; the generated code is byte-identical to a hand-written
//! per-module implementation.

/// Emit the `<View>` struct, its `Arena` accessor, and the full set
/// of safe zero-init alloc methods (`alloc`, `try_alloc`,
/// `alloc_slice`, `try_alloc_slice`, scalar/slice × `box`/`rc`/`arc`
/// variants) parameterized over the marker trait.
///
/// Parameters:
/// * `$View` — the public view struct name (`BytemuckView` or
///   `ZerocopyView`).
/// * `$Marker` — the trait path users must satisfy
///   (`bytemuck::Zeroable` or `zerocopy::FromZeros`).
/// * `$panic_msg` — the panic message string used by the cold
///   alloc-failure helper.
macro_rules! zero_init_view {
    ($View:ident, $Marker:path, $panic_msg:literal) => {
        /// Zero-cost view over an [`Arena`](crate::Arena) for safe zero-initialized allocation.
        ///
        /// Exposes safe zero-initialized allocation methods for types implementing
        /// the marker trait. Obtained via [`Arena`](crate::Arena)'s ecosystem-specific accessor.
        #[derive(Debug)]
        pub struct $View<'a, A: ::allocator_api2::alloc::Allocator + Clone = ::allocator_api2::alloc::Global> {
            arena: &'a $crate::Arena<A>,
        }

        impl<'a, A: ::allocator_api2::alloc::Allocator + Clone> $View<'a, A> {
            #[inline]
            pub(crate) const fn new(arena: &'a $crate::Arena<A>) -> Self {
                Self { arena }
            }

            /// Allocate a zero-initialized `T` and return a mutable reference into the arena.
            ///
            /// The returned reference's lifetime is tied to the arena.
            ///
            /// # Panics
            ///
            /// Panics if the backing allocator fails or if `T` requires alignment of 64 KiB or greater (which exceeds the arena chunk alignment).
            #[must_use]
            #[inline]
            pub fn alloc<T: $Marker>(&self) -> &'a mut T {
                match self.try_alloc::<T>() {
                    Ok(v) => v,
                    Err(_) => panic_alloc(),
                }
            }

            /// Fallible variant of [`Self::alloc`].
            ///
            /// # Errors
            ///
            /// Returns [`AllocError`](::allocator_api2::alloc::AllocError) if the backing allocator fails or if `T` requires alignment
            /// >= 64 KiB.
            #[inline]
            pub fn try_alloc<T: $Marker>(&self) -> Result<&'a mut T, ::allocator_api2::alloc::AllocError> {
                let uninit: &mut ::core::mem::MaybeUninit<T> = self.arena.try_alloc_with(::core::mem::MaybeUninit::<T>::zeroed)?;
                // SAFETY: the marker trait guarantees the all-zero bit pattern is a valid `T`.
                Ok(unsafe { ::core::mem::MaybeUninit::assume_init_mut(uninit) })
            }

            /// Allocate a zero-initialized slice of `T` and return a mutable slice into the arena.
            ///
            /// # Panics
            ///
            /// Panics if the backing allocator fails or if `T` requires alignment of 64 KiB or greater.
            #[must_use]
            #[inline]
            pub fn alloc_slice<T: $Marker>(&self, len: usize) -> &'a mut [T] {
                match self.try_alloc_slice::<T>(len) {
                    Ok(v) => v,
                    Err(_) => panic_alloc(),
                }
            }

            /// Fallible variant of [`Self::alloc_slice`].
            ///
            /// # Errors
            ///
            /// Returns [`AllocError`](::allocator_api2::alloc::AllocError) if the backing allocator fails or if `T` requires alignment
            /// >= 64 KiB.
            #[inline]
            pub fn try_alloc_slice<T: $Marker>(&self, len: usize) -> Result<&'a mut [T], ::allocator_api2::alloc::AllocError> {
                let slice: &mut [::core::mem::MaybeUninit<T>] = self
                    .arena
                    .try_alloc_slice_fill_with(len, |_| ::core::mem::MaybeUninit::<T>::zeroed())?;
                // SAFETY: the marker trait guarantees that the all-zero bit pattern is a valid `T`,
                // and the fill closure produced a slice of `len` zeroed elements.
                Ok(unsafe { &mut *(::core::ptr::from_mut::<[::core::mem::MaybeUninit<T>]>(slice) as *mut [T]) })
            }

            /// Allocate a zero-initialized `T` and return a [`Box<T, A>`](crate::Box).
            ///
            /// # Panics
            ///
            /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
            #[must_use]
            #[inline]
            pub fn alloc_box<T: $Marker>(&self) -> $crate::Box<T, A> {
                match self.try_alloc_box::<T>() {
                    Ok(v) => v,
                    Err(_) => panic_alloc(),
                }
            }

            /// Fallible variant of [`Self::alloc_box`].
            ///
            /// # Errors
            ///
            /// Returns [`AllocError`](::allocator_api2::alloc::AllocError) if the backing allocator fails or if `T` requires alignment
            /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
            #[inline]
            pub fn try_alloc_box<T: $Marker>(&self) -> Result<$crate::Box<T, A>, ::allocator_api2::alloc::AllocError> {
                let b = self.arena.try_alloc_zeroed_box::<T>()?;
                // SAFETY: the marker trait guarantees the all-zero bit pattern is a valid `T`.
                Ok(unsafe { b.assume_init() })
            }

            /// Allocate a zero-initialized `T` and return an [`Rc<T, A>`](crate::Rc).
            ///
            /// # Panics
            ///
            /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
            #[must_use]
            #[inline]
            pub fn alloc_rc<T: $Marker>(&self) -> $crate::Rc<T, A> {
                match self.try_alloc_rc::<T>() {
                    Ok(v) => v,
                    Err(_) => panic_alloc(),
                }
            }

            /// Fallible variant of [`Self::alloc_rc`].
            ///
            /// # Errors
            ///
            /// Returns [`AllocError`](::allocator_api2::alloc::AllocError) if the backing allocator fails or if `T` requires alignment
            /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
            #[inline]
            pub fn try_alloc_rc<T: $Marker>(&self) -> Result<$crate::Rc<T, A>, ::allocator_api2::alloc::AllocError> {
                let rc = self.arena.try_alloc_zeroed_rc::<T>()?;
                // SAFETY: the marker trait guarantees the all-zero bit pattern is a valid `T`.
                Ok(unsafe { rc.assume_init() })
            }

            /// Allocate a zero-initialized `T` and return an [`Arc<T, A>`](crate::Arc).
            ///
            /// # Panics
            ///
            /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
            #[must_use]
            #[inline]
            pub fn alloc_arc<T: $Marker + Send + Sync>(&self) -> $crate::Arc<T, A>
            where
                A: Send + Sync,
            {
                match self.try_alloc_arc::<T>() {
                    Ok(v) => v,
                    Err(_) => panic_alloc(),
                }
            }

            /// Fallible variant of [`Self::alloc_arc`].
            ///
            /// # Errors
            ///
            /// Returns [`AllocError`](::allocator_api2::alloc::AllocError) if the backing allocator fails or if `T` requires alignment
            /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
            #[inline]
            pub fn try_alloc_arc<T: $Marker + Send + Sync>(&self) -> Result<$crate::Arc<T, A>, ::allocator_api2::alloc::AllocError>
            where
                A: Send + Sync,
            {
                let arc = self.arena.try_alloc_zeroed_arc::<T>()?;
                // SAFETY: the marker trait guarantees the all-zero bit pattern is a valid `T`.
                Ok(unsafe { arc.assume_init() })
            }

            /// Allocate a zero-initialized slice of `T` and return an [`Rc<[T], A>`](crate::Rc).
            ///
            /// # Panics
            ///
            /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
            #[must_use]
            #[inline]
            pub fn alloc_slice_rc<T: $Marker>(&self, len: usize) -> $crate::Rc<[T], A> {
                match self.try_alloc_slice_rc::<T>(len) {
                    Ok(v) => v,
                    Err(_) => panic_alloc(),
                }
            }

            /// Fallible variant of [`Self::alloc_slice_rc`].
            ///
            /// # Errors
            ///
            /// Returns [`AllocError`](::allocator_api2::alloc::AllocError) if the backing allocator fails or if `T` requires alignment
            /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
            #[inline]
            pub fn try_alloc_slice_rc<T: $Marker>(&self, len: usize) -> Result<$crate::Rc<[T], A>, ::allocator_api2::alloc::AllocError> {
                let rc = self.arena.try_alloc_zeroed_slice_rc::<T>(len)?;
                // SAFETY: the marker trait guarantees the all-zero bit pattern is valid for every element.
                Ok(unsafe { rc.assume_init() })
            }

            /// Allocate a zero-initialized slice of `T` and return an [`Arc<[T], A>`](crate::Arc).
            ///
            /// # Panics
            ///
            /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
            #[must_use]
            #[inline]
            pub fn alloc_slice_arc<T: $Marker + Send + Sync>(&self, len: usize) -> $crate::Arc<[T], A>
            where
                A: Send + Sync,
            {
                match self.try_alloc_slice_arc::<T>(len) {
                    Ok(v) => v,
                    Err(_) => panic_alloc(),
                }
            }

            /// Fallible variant of [`Self::alloc_slice_arc`].
            ///
            /// # Errors
            ///
            /// Returns [`AllocError`](::allocator_api2::alloc::AllocError) if the backing allocator fails or if `T` requires alignment
            /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
            #[inline]
            pub fn try_alloc_slice_arc<T: $Marker + Send + Sync>(
                &self,
                len: usize,
            ) -> Result<$crate::Arc<[T], A>, ::allocator_api2::alloc::AllocError>
            where
                A: Send + Sync,
            {
                let arc = self.arena.try_alloc_zeroed_slice_arc::<T>(len)?;
                // SAFETY: the marker trait guarantees the all-zero bit pattern is valid for every element.
                Ok(unsafe { arc.assume_init() })
            }

            /// Allocate a zero-initialized slice of `T` and return a [`Box<[T], A>`](crate::Box).
            ///
            /// # Panics
            ///
            /// Panics if the backing allocator fails or if `T` requires alignment of 32 KiB or greater (smart-pointer paths cap alignment at half the chunk alignment).
            #[must_use]
            #[inline]
            pub fn alloc_slice_box<T: $Marker>(&self, len: usize) -> $crate::Box<[T], A> {
                match self.try_alloc_slice_box::<T>(len) {
                    Ok(v) => v,
                    Err(_) => panic_alloc(),
                }
            }

            /// Fallible variant of [`Self::alloc_slice_box`].
            ///
            /// # Errors
            ///
            /// Returns [`AllocError`](::allocator_api2::alloc::AllocError) if the backing allocator fails or if `T` requires alignment
            /// >= 32 KiB (smart-pointer paths cap alignment at half the chunk alignment).
            #[inline]
            pub fn try_alloc_slice_box<T: $Marker>(&self, len: usize) -> Result<$crate::Box<[T], A>, ::allocator_api2::alloc::AllocError> {
                let b = self.arena.try_alloc_zeroed_slice_box::<T>(len)?;
                // SAFETY: the marker trait guarantees the all-zero bit pattern is valid for every element.
                Ok(unsafe { b.assume_init() })
            }
        }

        #[cold]
        #[inline(never)]
        #[allow(
            clippy::panic,
            reason = "intentional panic on allocation failure matching Arena's own panic_alloc"
        )]
        fn panic_alloc() -> ! {
            panic!($panic_msg);
        }
    };
}

pub(crate) use zero_init_view;
