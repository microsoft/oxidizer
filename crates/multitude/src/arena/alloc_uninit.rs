// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uninitialized / zeroed allocation API on [`Arena`].
//!
//! All public methods are documented on [`Arena`] itself; this file
//! groups the `alloc_uninit_*` / `alloc_zeroed_*` family together to
//! keep the central `mod.rs` smaller.

use core::mem::MaybeUninit;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::{AllocFlavor, Arena, expect_alloc};
use crate::arc::Arc;
use crate::r#box::Box;
use crate::internal::drop_list::noop_drop_shim;
use crate::rc::Rc;

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate uninitialized space for a `T` and return an
    /// [`Box<MaybeUninit<T>, A>`](crate::Box). The caller must
    /// initialize the value (e.g., via [`MaybeUninit::write`]) before
    /// calling [`Box::<MaybeUninit<T>, A>::assume_init`].
    ///
    /// For types that need drop, this allocation reserves a `DropEntry`
    /// slot in addition to the value bytes. Dropping the box without
    /// `assume_init` is sound (no destructor runs).
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_box`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_box<T>(&self) -> Box<MaybeUninit<T>, A> {
        // Reuse the slice path so drop-aware types still reserve a noop entry.
        // The closure-based init keeps over-aligned T's stack-materialized
        // value out of the caller's frame, unlike a `try_alloc_inner_value`
        // fast path which would require passing `MaybeUninit<T>` by value
        // (and on Windows-under-coverage that materializes a 64 KiB-aligned
        // slot in the caller's 1 MiB stack — see the `OverAligned` notes in
        // `tests/bytemuck_integration.rs`).
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let slice = self.alloc_slice_local_with_or_panic(1, AllocFlavor::Box, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
            dst.write(MaybeUninit::uninit());
        });
        let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
        // SAFETY: helper allocated one MaybeUninit<T> and bumped the chunk refcount for this Box.
        unsafe { Box::from_raw(NonNull::new_unchecked(ptr)) }
    }

    /// Fallible variant of [`Self::alloc_uninit_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_box<T>(&self) -> Result<Box<MaybeUninit<T>, A>, AllocError> {
        // Reuse the slice path with `len = 1` so drop-aware types reserve
        // the noop entry that `assume_init().into_rc()` later retargets.
        // See `alloc_uninit_box` for why the value-path fast path is unsafe
        // for over-aligned T on Windows.
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let slice =
            self.try_alloc_slice_local_with::<_, _, false>(1, AllocFlavor::Box, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::uninit());
            })?;
        let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
        // SAFETY: helper allocated one MaybeUninit<T> and bumped the chunk refcount for this Box.
        Ok(unsafe { Box::from_raw(NonNull::new_unchecked(ptr)) })
    }

    /// Like [`Self::alloc_uninit_box`] but the value bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_box`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_box<T>(&self) -> Box<MaybeUninit<T>, A> {
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let slice = self.alloc_slice_local_with_or_panic(1, AllocFlavor::Box, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
            dst.write(MaybeUninit::zeroed());
        });
        let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
        // SAFETY: helper allocated one MaybeUninit<T> and bumped the chunk refcount for this Box.
        unsafe { Box::from_raw(NonNull::new_unchecked(ptr)) }
    }

    /// Fallible variant of [`Self::alloc_zeroed_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_zeroed_box<T>(&self) -> Result<Box<MaybeUninit<T>, A>, AllocError> {
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let slice =
            self.try_alloc_slice_local_with::<_, _, false>(1, AllocFlavor::Box, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::zeroed());
            })?;
        let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
        // SAFETY: helper allocated one MaybeUninit<T> and bumped the chunk refcount for this Box.
        Ok(unsafe { Box::from_raw(NonNull::new_unchecked(ptr)) })
    }

    /// Allocate uninitialized space for a `T` and return an
    /// [`Rc<MaybeUninit<T>, A>`](crate::Rc).
    ///
    /// For types that need drop, this allocation reserves a `DropEntry`
    /// slot wired to a placeholder shim. Dropping the
    /// `Rc<MaybeUninit<T>>` without
    /// `Rc::<MaybeUninit<T>, A>::assume_init` is sound — the
    /// placeholder shim is a no-op, so `T::drop` does not run on
    /// uninitialized memory. `assume_init` rewrites the shim to
    /// `drop_shim::<T>` so a subsequent `Rc<T>` drops the value
    /// correctly.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_rc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_rc<T>(&self) -> Rc<MaybeUninit<T>, A> {
        // Reuse the slice path so drop-aware types still reserve a noop entry.
        // See `alloc_uninit_box` for why the value-path fast path is unsafe
        // for over-aligned T on Windows-under-coverage.
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let slice = self.alloc_slice_local_with_or_panic(1, AllocFlavor::Rc, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
            dst.write(MaybeUninit::uninit());
        });
        let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
        // SAFETY: helper initialized one MaybeUninit<T> and bumped the refcount for this Rc.
        unsafe { Rc::from_value_ptr(NonNull::new_unchecked(ptr)) }
    }

    /// Fallible variant of [`Self::alloc_uninit_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_rc<T>(&self) -> Result<Rc<MaybeUninit<T>, A>, AllocError> {
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let slice =
            self.try_alloc_slice_local_with::<_, _, false>(1, AllocFlavor::Rc, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::uninit());
            })?;
        let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
        // SAFETY: helper initialized one MaybeUninit<T> and bumped the refcount for this Rc.
        Ok(unsafe { Rc::from_value_ptr(NonNull::new_unchecked(ptr)) })
    }

    /// Like [`Self::alloc_uninit_rc`] but the value bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_rc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_rc<T>(&self) -> Rc<MaybeUninit<T>, A> {
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let slice = self.alloc_slice_local_with_or_panic(1, AllocFlavor::Rc, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
            dst.write(MaybeUninit::zeroed());
        });
        let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
        // SAFETY: helper initialized one MaybeUninit<T> and bumped the refcount for this Rc.
        unsafe { Rc::from_value_ptr(NonNull::new_unchecked(ptr)) }
    }

    /// Fallible variant of [`Self::alloc_zeroed_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_zeroed_rc<T>(&self) -> Result<Rc<MaybeUninit<T>, A>, AllocError> {
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let slice =
            self.try_alloc_slice_local_with::<_, _, false>(1, AllocFlavor::Rc, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::zeroed());
            })?;
        let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
        // SAFETY: helper initialized one MaybeUninit<T> and bumped the refcount for this Rc.
        Ok(unsafe { Rc::from_value_ptr(NonNull::new_unchecked(ptr)) })
    }

    /// Allocate uninitialized space for a `T` and return an
    /// [`Arc<MaybeUninit<T>, A>`](crate::Arc).
    ///
    /// For types that need drop, this allocation reserves a `DropEntry`
    /// slot wired to a placeholder shim. Dropping the
    /// `Arc<MaybeUninit<T>>` without
    /// `Arc::<MaybeUninit<T>, A>::assume_init` is sound — the
    /// placeholder shim is a no-op, so `T::drop` does not run on
    /// uninitialized memory. `assume_init` rewrites the shim to
    /// `drop_shim::<T>` so a subsequent `Arc<T>` drops the value
    /// correctly.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_arc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_arc<T>(&self) -> Arc<MaybeUninit<T>, A>
    where
        A: Send + Sync,
    {
        if const { core::mem::needs_drop::<T>() } {
            let drop_fn = Some(noop_drop_shim as unsafe fn(*mut u8, usize));
            let slice = self.alloc_slice_shared_with_or_panic(1, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::uninit());
            });
            let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
            // SAFETY: helper initialized one MaybeUninit<T> and accounted for this Arc.
            unsafe { Arc::from_value_ptr(NonNull::new_unchecked(ptr)) }
        } else {
            // No-drop T: bypass the slice path's per-slot loop by reusing
            // the Arc value path. `needs_drop::<MaybeUninit<T>>() == false`
            // ⇒ no entry reserved; the closure's `MaybeUninit::uninit()`
            // construction compiles to a no-op.
            let ptr = self.alloc_inner_arc_with_or_panic::<MaybeUninit<T>, _>(MaybeUninit::uninit);
            // SAFETY: helper initialized one MaybeUninit<T> and accounted for this Arc.
            unsafe { Arc::from_value_ptr(ptr) }
        }
    }

    /// Fallible variant of [`Self::alloc_uninit_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_arc<T>(&self) -> Result<Arc<MaybeUninit<T>, A>, AllocError>
    where
        A: Send + Sync,
    {
        if const { core::mem::needs_drop::<T>() } {
            let drop_fn = Some(noop_drop_shim as unsafe fn(*mut u8, usize));
            let slice = self.try_alloc_slice_shared_with::<_, _, false>(1, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::uninit());
            })?;
            let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
            // SAFETY: helper initialized one MaybeUninit<T> and accounted for this Arc.
            Ok(unsafe { Arc::from_value_ptr(NonNull::new_unchecked(ptr)) })
        } else {
            let ptr = self.try_alloc_inner_arc_with::<MaybeUninit<T>, _>(MaybeUninit::uninit)?;
            // SAFETY: helper initialized one MaybeUninit<T> and accounted for this Arc.
            Ok(unsafe { Arc::from_value_ptr(ptr) })
        }
    }

    /// Like [`Self::alloc_uninit_arc`] but the value bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_arc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_arc<T>(&self) -> Arc<MaybeUninit<T>, A>
    where
        A: Send + Sync,
    {
        if const { core::mem::needs_drop::<T>() } {
            let drop_fn = Some(noop_drop_shim as unsafe fn(*mut u8, usize));
            let slice = self.alloc_slice_shared_with_or_panic(1, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::zeroed());
            });
            let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
            // SAFETY: helper initialized one MaybeUninit<T> and accounted for this Arc.
            unsafe { Arc::from_value_ptr(NonNull::new_unchecked(ptr)) }
        } else {
            let ptr = self.alloc_inner_arc_with_or_panic::<MaybeUninit<T>, _>(MaybeUninit::zeroed);
            // SAFETY: helper initialized one MaybeUninit<T> and accounted for this Arc.
            unsafe { Arc::from_value_ptr(ptr) }
        }
    }

    /// Fallible variant of [`Self::alloc_zeroed_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_zeroed_arc<T>(&self) -> Result<Arc<MaybeUninit<T>, A>, AllocError>
    where
        A: Send + Sync,
    {
        if const { core::mem::needs_drop::<T>() } {
            let drop_fn = Some(noop_drop_shim as unsafe fn(*mut u8, usize));
            let slice = self.try_alloc_slice_shared_with::<_, _, false>(1, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::zeroed());
            })?;
            let ptr = slice.as_ptr().cast::<MaybeUninit<T>>();
            // SAFETY: helper initialized one MaybeUninit<T> and accounted for this Arc.
            Ok(unsafe { Arc::from_value_ptr(NonNull::new_unchecked(ptr)) })
        } else {
            let ptr = self.try_alloc_inner_arc_with::<MaybeUninit<T>, _>(MaybeUninit::zeroed)?;
            // SAFETY: helper initialized one MaybeUninit<T> and accounted for this Arc.
            Ok(unsafe { Arc::from_value_ptr(ptr) })
        }
    }

    /// Allocate `len` uninitialized `T` slots and return an
    /// [`Rc<[MaybeUninit<T>], A>`](crate::Rc).
    ///
    /// For element types that need drop, this allocation reserves a
    /// slice-`DropEntry` slot wired to a placeholder shim. Dropping the
    /// `Rc<[MaybeUninit<T>]>` without
    /// [`Rc::<[MaybeUninit<T>], A>::assume_init`] is sound — the
    /// placeholder shim is a no-op, so `T::drop` does not run on
    /// uninitialized elements. `assume_init` rewrites the shim to
    /// `slice_drop_shim::<T>` so the elements drop correctly via the
    /// resulting `Rc<[T]>`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_slice_rc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_slice_rc<T>(&self, len: usize) -> Rc<[MaybeUninit<T>], A> {
        expect_alloc(self.try_alloc_uninit_slice_rc(len))
    }

    /// Fallible variant of [`Self::alloc_uninit_slice_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_slice_rc<T>(&self, len: usize) -> Result<Rc<[MaybeUninit<T>], A>, AllocError> {
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let ptr =
            self.try_alloc_slice_local_with::<_, _, false>(len, AllocFlavor::Rc, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::uninit());
            })?;
        // SAFETY: helper initialized all MaybeUninit slots and bumped the refcount for this Rc.
        Ok(unsafe { Rc::from_value_ptr(ptr) })
    }

    /// Like [`Self::alloc_uninit_slice_rc`] but the slice bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_slice_rc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_slice_rc<T>(&self, len: usize) -> Rc<[MaybeUninit<T>], A> {
        expect_alloc(self.try_alloc_zeroed_slice_rc(len))
    }

    /// Fallible variant of [`Self::alloc_zeroed_slice_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_zeroed_slice_rc<T>(&self, len: usize) -> Result<Rc<[MaybeUninit<T>], A>, AllocError> {
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let ptr =
            self.try_alloc_slice_local_with::<_, _, false>(len, AllocFlavor::Rc, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::zeroed());
            })?;
        // SAFETY: helper initialized all MaybeUninit slots and bumped the refcount for this Rc.
        Ok(unsafe { Rc::from_value_ptr(ptr) })
    }

    /// Allocate `len` uninitialized `T` slots and return an
    /// [`Arc<[MaybeUninit<T>], A>`](crate::Arc).
    ///
    /// For element types that need drop, this allocation reserves a
    /// slice-`DropEntry` slot wired to a placeholder shim. Dropping the
    /// `Arc<[MaybeUninit<T>]>` without
    /// [`Arc::<[MaybeUninit<T>], A>::assume_init`] is sound — the
    /// placeholder shim is a no-op, so `T::drop` does not run on
    /// uninitialized elements. `assume_init` rewrites the shim to
    /// `slice_drop_shim::<T>` so the elements drop correctly via the
    /// resulting `Arc<[T]>`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_slice_arc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_slice_arc<T>(&self, len: usize) -> Arc<[MaybeUninit<T>], A>
    where
        A: Send + Sync,
    {
        expect_alloc(self.try_alloc_uninit_slice_arc(len))
    }

    /// Fallible variant of [`Self::alloc_uninit_slice_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_slice_arc<T>(&self, len: usize) -> Result<Arc<[MaybeUninit<T>], A>, AllocError>
    where
        A: Send + Sync,
    {
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let ptr = self.try_alloc_slice_shared_with::<_, _, false>(len, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
            dst.write(MaybeUninit::uninit());
        })?;
        // SAFETY: helper initialized all MaybeUninit slots and accounted for this Arc.
        Ok(unsafe { Arc::from_value_ptr(ptr) })
    }

    /// Like [`Self::alloc_uninit_slice_arc`] but the slice bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_slice_arc`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_slice_arc<T>(&self, len: usize) -> Arc<[MaybeUninit<T>], A>
    where
        A: Send + Sync,
    {
        expect_alloc(self.try_alloc_zeroed_slice_arc(len))
    }

    /// Fallible variant of [`Self::alloc_zeroed_slice_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_zeroed_slice_arc<T>(&self, len: usize) -> Result<Arc<[MaybeUninit<T>], A>, AllocError>
    where
        A: Send + Sync,
    {
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let ptr = self.try_alloc_slice_shared_with::<_, _, false>(len, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
            dst.write(MaybeUninit::zeroed());
        })?;
        // SAFETY: helper initialized all MaybeUninit slots and accounted for this Arc.
        Ok(unsafe { Arc::from_value_ptr(ptr) })
    }

    /// Allocate `len` uninitialized `T` slots and return an
    /// [`Box<[MaybeUninit<T>], A>`](crate::Box).
    ///
    /// For element types that need drop, this allocation reserves a
    /// slice-`DropEntry` slot wired to a placeholder shim. Dropping the
    /// `Box<[MaybeUninit<T>]>` without
    /// [`Box::<[MaybeUninit<T>], A>::assume_init`] is sound — the
    /// placeholder shim is a no-op, so `T::drop` does not run on
    /// uninitialized elements. `assume_init` rewrites the shim to
    /// `slice_drop_shim::<T>` so the elements drop correctly via the
    /// resulting `Box<[T]>`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_uninit_slice_box`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_slice_box<T>(&self, len: usize) -> Box<[MaybeUninit<T>], A> {
        expect_alloc(self.try_alloc_uninit_slice_box(len))
    }

    /// Fallible variant of [`Self::alloc_uninit_slice_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_slice_box<T>(&self, len: usize) -> Result<Box<[MaybeUninit<T>], A>, AllocError> {
        // Reserve a `noop_drop_shim` slot for `T: needs_drop` so that
        // `Box<[MaybeUninit<T>]>::assume_init().into_rc()` has an entry
        // to retarget. See `try_alloc_uninit_box` for full rationale.
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let ptr =
            self.try_alloc_slice_local_with::<_, _, false>(len, AllocFlavor::Box, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::uninit());
            })?;
        // SAFETY: helper initialized all MaybeUninit slots and bumped the refcount for this Box.
        Ok(unsafe { Box::from_raw_unsized(ptr) })
    }

    /// Like [`Self::alloc_uninit_slice_box`] but the slice bytes are zeroed.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_zeroed_slice_box`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_slice_box<T>(&self, len: usize) -> Box<[MaybeUninit<T>], A> {
        expect_alloc(self.try_alloc_zeroed_slice_box(len))
    }

    /// Fallible variant of [`Self::alloc_zeroed_slice_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_zeroed_slice_box<T>(&self, len: usize) -> Result<Box<[MaybeUninit<T>], A>, AllocError> {
        // See `try_alloc_uninit_slice_box` for rationale.
        let drop_fn = const { core::mem::needs_drop::<T>() }.then_some(noop_drop_shim as unsafe fn(*mut u8, usize));
        let ptr =
            self.try_alloc_slice_local_with::<_, _, false>(len, AllocFlavor::Box, drop_fn, |_, dst: &mut MaybeUninit<MaybeUninit<T>>| {
                dst.write(MaybeUninit::zeroed());
            })?;
        // SAFETY: helper initialized all MaybeUninit slots and bumped the refcount for this Box.
        Ok(unsafe { Box::from_raw_unsized(ptr) })
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate an uninitialized `MaybeUninit<T>` slot and return a
    /// [`Pin<Box<MaybeUninit<T>, A>>`](core::pin::Pin). Pair with
    /// [`Box::assume_init_pin`](crate::Box::assume_init_pin) once
    /// initialization completes to obtain `Pin<Box<T, A>>` without
    /// ever moving the value off-arena.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `align_of::<T>()`
    /// is at least 32 KiB.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_box_pin<T>(&self) -> core::pin::Pin<Box<MaybeUninit<T>, A>>
    where
        A: 'static,
    {
        Box::into_pin(self.alloc_uninit_box::<T>())
    }

    /// Fallible variant of [`Self::alloc_uninit_box_pin`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    #[inline]
    pub fn try_alloc_uninit_box_pin<T>(&self) -> Result<core::pin::Pin<Box<MaybeUninit<T>, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_uninit_box::<T>().map(Box::into_pin)
    }

    /// Zeroed pinned uninit variant of [`Self::alloc_uninit_box_pin`].
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_box_pin<T>(&self) -> core::pin::Pin<Box<MaybeUninit<T>, A>>
    where
        A: 'static,
    {
        Box::into_pin(self.alloc_zeroed_box::<T>())
    }

    /// Fallible variant of [`Self::alloc_zeroed_box_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[inline]
    pub fn try_alloc_zeroed_box_pin<T>(&self) -> Result<core::pin::Pin<Box<MaybeUninit<T>, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_zeroed_box::<T>().map(Box::into_pin)
    }

    /// `Rc` mirror of [`Self::alloc_uninit_box_pin`].
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[must_use]
    #[inline]
    pub fn alloc_uninit_rc_pin<T>(&self) -> core::pin::Pin<Rc<MaybeUninit<T>, A>>
    where
        A: 'static,
    {
        Rc::into_pin(self.alloc_uninit_rc::<T>())
    }

    /// Fallible variant of [`Self::alloc_uninit_rc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[inline]
    pub fn try_alloc_uninit_rc_pin<T>(&self) -> Result<core::pin::Pin<Rc<MaybeUninit<T>, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_uninit_rc::<T>().map(Rc::into_pin)
    }

    /// `Rc` mirror of [`Self::alloc_zeroed_box_pin`].
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_rc_pin<T>(&self) -> core::pin::Pin<Rc<MaybeUninit<T>, A>>
    where
        A: 'static,
    {
        Rc::into_pin(self.alloc_zeroed_rc::<T>())
    }

    /// Fallible variant of [`Self::alloc_zeroed_rc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[inline]
    pub fn try_alloc_zeroed_rc_pin<T>(&self) -> Result<core::pin::Pin<Rc<MaybeUninit<T>, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_zeroed_rc::<T>().map(Rc::into_pin)
    }

    /// `Arc` mirror of [`Self::alloc_uninit_box_pin`].
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[must_use]
    #[inline]
    pub fn alloc_uninit_arc_pin<T>(&self) -> core::pin::Pin<Arc<MaybeUninit<T>, A>>
    where
        A: Send + Sync + 'static,
    {
        Arc::into_pin(self.alloc_uninit_arc::<T>())
    }

    /// Fallible variant of [`Self::alloc_uninit_arc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[inline]
    pub fn try_alloc_uninit_arc_pin<T>(&self) -> Result<core::pin::Pin<Arc<MaybeUninit<T>, A>>, AllocError>
    where
        A: Send + Sync + 'static,
    {
        self.try_alloc_uninit_arc::<T>().map(Arc::into_pin)
    }

    /// `Arc` mirror of [`Self::alloc_zeroed_box_pin`].
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[must_use]
    #[inline]
    pub fn alloc_zeroed_arc_pin<T>(&self) -> core::pin::Pin<Arc<MaybeUninit<T>, A>>
    where
        A: Send + Sync + 'static,
    {
        Arc::into_pin(self.alloc_zeroed_arc::<T>())
    }

    /// Fallible variant of [`Self::alloc_zeroed_arc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::alloc_uninit_box_pin`].
    #[inline]
    pub fn try_alloc_zeroed_arc_pin<T>(&self) -> Result<core::pin::Pin<Arc<MaybeUninit<T>, A>>, AllocError>
    where
        A: Send + Sync + 'static,
    {
        self.try_alloc_zeroed_arc::<T>().map(Arc::into_pin)
    }
}
