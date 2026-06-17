// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Box<[T]>` slice allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.

use core::mem;
use core::pin::Pin;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::{AllocError, Allocator};

use super::alloc_prefixed::worst_case_thin_slice_payload;
use super::alloc_value::{MAX_SMART_PTR_ALIGN, acquire_shared_chunk_ref};
use super::{Arena, ExpectAlloc};
use crate::r#box::Box;

impl<A: Allocator + Clone> Arena<A> {
    /// Copy `slice` into the arena and return a [`Box<[T], A>`](crate::Box).
    ///
    /// The returned smart pointer is owned and mutable; its `Drop` runs
    /// `T::drop` on each element immediately when the smart pointer is
    /// dropped.
    ///
    /// Available only with the `dst` Cargo feature, which pulls in the
    /// `ptr_meta` crate to polyfill stable `ptr::metadata`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_copy_box`] for a fallible variant.
    #[must_use]
    #[inline]
    pub fn alloc_slice_copy_box<T: Copy>(&self, slice: impl AsRef<[T]>) -> Box<[T], A> {
        let s = slice.as_ref();
        (self.impl_alloc_slice_box_copy::<T>(s)).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_copy_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_slice_copy_box<T: Copy>(&self, slice: impl AsRef<[T]>) -> Result<Box<[T], A>, AllocError> {
        self.impl_alloc_slice_box_copy::<T>(slice.as_ref())
    }

    /// Clone every element of `slice` into the arena and return an
    /// owned, mutable [`Box<[T], A>`](crate::Box).
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_clone_box`] for a fallible variant.
    ///
    /// May panic if `T::clone` panics; already-cloned elements are dropped before the
    /// panic propagates.
    #[inline]
    pub fn alloc_slice_clone_box<T: Clone>(&self, slice: impl AsRef<[T]>) -> Box<[T], A> {
        let s = slice.as_ref();
        (self.impl_alloc_slice_box_with::<T, _>(s.len(), |i| s[i].clone())).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_clone_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// May panic if `T::clone` panics; already-cloned elements are
    /// dropped before the panic propagates.
    #[inline]
    pub fn try_alloc_slice_clone_box<T: Clone>(&self, slice: impl AsRef<[T]>) -> Result<Box<[T], A>, AllocError> {
        let s = slice.as_ref();
        self.impl_alloc_slice_box_with::<T, _>(s.len(), |i| s[i].clone())
    }

    /// Allocate a slice of `len` elements, with element `i` produced by `f(i)`.
    ///
    /// Returns an owned, mutable [`Box<[T], A>`](crate::Box).
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_with_box`] for a fallible variant.
    ///
    /// If `f` panics, already-initialized elements are dropped (drop guard) and the
    /// panic propagates.
    #[inline]
    pub fn alloc_slice_fill_with_box<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Box<[T], A> {
        (self.impl_alloc_slice_box_with::<T, F>(len, f)).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// If `f` panics, already-initialized elements are dropped and the
    /// panic propagates.
    #[inline]
    pub fn try_alloc_slice_fill_with_box<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<Box<[T], A>, AllocError> {
        self.impl_alloc_slice_box_with::<T, F>(len, f)
    }

    /// Allocate a slice and fill it with values pulled from `iter`.
    ///
    /// Returns an owned, mutable [`Box<[T], A>`](crate::Box).
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_iter_box`] for a fallible variant.
    ///
    /// May also panic if the iterator yields fewer elements than its
    /// `ExactSizeIterator::len()` reported.
    #[inline]
    pub fn alloc_slice_fill_iter_box<T, I>(&self, iter: I) -> Box<[T], A>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let it = iter.into_iter();
        let len = it.len();
        (self.impl_alloc_slice_box_iter::<T, _>(len, it)).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_fill_iter_box`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// Panics if the iterator yields fewer elements than its
    /// `ExactSizeIterator::len()` reported.
    #[inline]
    pub fn try_alloc_slice_fill_iter_box<T, I>(&self, iter: I) -> Result<Box<[T], A>, AllocError>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let it = iter.into_iter();
        let len = it.len();
        self.impl_alloc_slice_box_iter::<T, _>(len, it)
    }

    /// Box: no chunk drop entry (`Box::drop` runs `drop_in_place` on the
    /// slice eagerly). Copy fast path.
    #[inline]
    fn impl_alloc_slice_box_copy<T: Copy>(&self, src: &[T]) -> Result<Box<[T], A>, AllocError> {
        check_slice_box_layout::<T>(src.len())?;
        let len = src.len();
        // `src` is a live `&[T]`, so `size_of_val(src)` is a valid
        // `usize`. Hoisting it past the refill loop spares the inner
        // reservation a `checked_mul` overflow guard.
        let payload_bytes = mem::size_of_val(src);
        let ptr = self.reserve_slice_box::<T>(len, payload_bytes, |slot_ptr| {
            // SAFETY: `slot_ptr` is the reservation start; `len` elements
            // of `T` fit by construction.
            unsafe { ptr::copy_nonoverlapping(src.as_ptr(), slot_ptr, len) };
        })?;
        // `ptr` points to `len` initialized `T`s in a shared chunk that
        // has a fresh +1; `Box::from_raw` adopts that +1 and `Box::drop` runs
        // `drop_in_place` on the slice when the smart pointer is dropped.
        // SAFETY: see above.
        Ok(unsafe { Box::from_raw(ptr.cast::<u8>()) })
    }

    /// Box: with-closure fill path. Uses an `InitGuard`-equivalent loop
    /// so a panicking `f` drops the already-initialized prefix.
    #[inline]
    fn impl_alloc_slice_box_with<T, F: FnMut(usize) -> T>(&self, len: usize, mut f: F) -> Result<Box<[T], A>, AllocError> {
        check_slice_box_layout::<T>(len)?;
        // Caller-provided `len`: must overflow-check the payload size
        // up front so the hot loop can skip the `checked_mul`. On
        // overflow we report `AllocError` immediately rather than spin
        // refilling.
        let payload_bytes = mem::size_of::<T>().checked_mul(len).ok_or(AllocError)?;
        let ptr = self.reserve_slice_box::<T>(len, payload_bytes, |slot_ptr| {
            // SAFETY: `slot_ptr` is the reservation start; we init `len` slots
            // with panic-safe rollback via `InitGuard`.
            unsafe {
                let mut guard = InitGuard {
                    dst: slot_ptr,
                    initialized: 0,
                };
                while guard.initialized < len {
                    slot_ptr.add(guard.initialized).write(f(guard.initialized));
                    guard.initialized += 1;
                }
                mem::forget(guard);
            }
        })?;
        // SAFETY: see `impl_alloc_slice_box_copy`.
        Ok(unsafe { Box::from_raw(ptr.cast::<u8>()) })
    }

    /// Box: iter-fill path (delegates to `_with`).
    #[inline]
    fn impl_alloc_slice_box_iter<T, I: Iterator<Item = T>>(&self, len: usize, mut iter: I) -> Result<Box<[T], A>, AllocError> {
        self.impl_alloc_slice_box_with::<T, _>(len, move |_| {
            iter.next()
                .expect("caller violated ExactSizeIterator contract: iterator yielded fewer elements than reported")
        })
    }

    /// Reserve `len` `T` slots (with precomputed `payload_bytes ==
    /// size_of::<T>() * len`) in the current shared chunk, bump the
    /// chunk's strong refcount, call `init(slot_ptr)`, and return the
    /// base pointer on success. On allocator failure, refills and
    /// retries; on `init` panic, the refcount bump is released via
    /// `ChunkRef::Drop` (reservation is leaked in-chunk).
    #[inline]
    fn reserve_slice_box<T>(&self, len: usize, payload_bytes: usize, init: impl FnOnce(*mut T)) -> Result<NonNull<T>, AllocError> {
        debug_assert_eq!(payload_bytes, mem::size_of::<T>().wrapping_mul(len));
        // Width budget includes prefix + payload alignment slack +
        // payload bytes.
        let bytes_needed = worst_case_thin_slice_payload::<T>(len);
        let mut init = Some(init);
        loop {
            // SAFETY: `payload_bytes == size_of::<T>() * len` per caller contract.
            let reserved = unsafe { self.try_reserve_shared_slice_with_size::<T>(len, payload_bytes) };
            if let Some((uninit, chunk_ptr)) = reserved {
                let chunk_ref = self.acquire_current_shared_chunk_ref(chunk_ptr);
                let (base, _len) = uninit.into_raw_buffer();
                // Run the init under the chunk_ref's Drop guard: a panic
                // releases the +1 so the chunk is not leaked.
                let init = init.take().expect("reserve_slice_box init taken twice");
                init(base.as_ptr());
                let _ = chunk_ref.forget();
                return Ok(base);
            }
            if self.is_oversized(bytes_needed) {
                let init_owned = init.take().expect("reserve_slice_box init taken twice");
                return self.alloc_oversized_shared_with(bytes_needed, |mutator, chunk_ptr| {
                    let ticket = mutator
                        .try_alloc_uninit_slice_prefixed::<T>(len)
                        .expect("dedicated oversized chunk sized to fit slice");
                    let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                    let (base, _len) = ticket.into_raw_buffer();
                    init_owned(base.as_ptr());
                    let _ = chunk_ref.forget();
                    base
                });
            }
            self.refill_shared(bytes_needed)?;
        }
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate `len` slots and fill each via `f(i)`, returning a
    /// [`Pin<Box<[T], A>>`](core::pin::Pin). Each element is pinned
    /// to its slot.
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_slice_fill_with_box`].
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_with_box_pin<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Pin<Box<[T], A>>
    where
        A: 'static,
    {
        Box::into_pin(self.alloc_slice_fill_with_box::<T, F>(len, f))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_box_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_slice_fill_with_box`].
    #[inline]
    pub fn try_alloc_slice_fill_with_box_pin<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<Pin<Box<[T], A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_slice_fill_with_box::<T, F>(len, f).map(Box::into_pin)
    }
}

/// Common up-front checks for the `Box<[T]>` slice family. `Box::drop`
/// runs `drop_in_place` on the entire slice eagerly, so no chunk drop
/// entry is recorded; however we still reject `T: Drop` slices with
/// `len > u16::MAX` so a future `Box<[T]> -> Arc<[T]>` conversion has
/// a slot to populate (parity with the `alloc_dst_box` guard).
//
// Mutation testing is suppressed: bypassing the `len > u16::MAX`
// rejection sends the caller's refill loop into an unbounded
// chunk-allocation spin (see `alloc_slice_ref::reject_drop_slice_too_long`).
#[cfg_attr(test, mutants::skip)]
#[inline]
fn check_slice_box_layout<T>(len: usize) -> Result<(), AllocError> {
    if mem::align_of::<T>() >= MAX_SMART_PTR_ALIGN {
        return Err(AllocError);
    }
    if mem::needs_drop::<T>() && len > u16::MAX as usize {
        return Err(AllocError);
    }
    Ok(())
}

/// Drop-guard for partial init in `alloc_slice_*_box`. Mirrors the
/// `InitGuard` in `internal::uninit`.
struct InitGuard<T> {
    dst: *mut T,
    initialized: usize,
}

impl<T> Drop for InitGuard<T> {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: `dst[..initialized]` were written by successful producer
        // calls; no other reference to those slots exists.
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.dst, self.initialized));
        }
    }
}
