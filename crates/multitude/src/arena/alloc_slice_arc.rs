// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Arc<[T]>` slice allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.

use core::mem;
use core::pin::Pin;

use allocator_api2::alloc::{AllocError, Allocator};

use super::alloc_prefixed::worst_case_thin_slice_payload;
use super::alloc_value::{MAX_SMART_PTR_ALIGN, acquire_shared_chunk_ref};
use super::{Arena, ExpectAlloc};
use crate::arc::Arc;

impl<A: Allocator + Clone> Arena<A> {
    /// Copy `slice` into a `Shared`-flavor chunk and return an [`Arc`].
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_copy_arc`] for a fallible variant.
    #[inline]
    pub fn alloc_slice_copy_arc<T: Copy + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Arc<[T], A>
    where
        A: Send + Sync,
    {
        let s = slice.as_ref();
        (self.impl_alloc_slice_arc_copy::<T>(s)).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_copy_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_slice_copy_arc<T: Copy + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Result<Arc<[T], A>, AllocError>
    where
        A: Send + Sync,
    {
        self.impl_alloc_slice_arc_copy::<T>(slice.as_ref())
    }

    /// Clone every element of `slice` into a `Shared`-flavor chunk and return an [`Arc`].
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// May panic if `T::clone` panics; already-cloned elements are dropped before the
    /// panic propagates.
    #[inline]
    pub fn alloc_slice_clone_arc<T: Clone + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Arc<[T], A>
    where
        A: Send + Sync,
    {
        let s = slice.as_ref();
        (self.impl_alloc_slice_arc_with::<T, _>(s.len(), |i| s[i].clone())).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_clone_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_slice_clone_arc<T: Clone + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Result<Arc<[T], A>, AllocError>
    where
        A: Send + Sync,
    {
        let s = slice.as_ref();
        self.impl_alloc_slice_arc_with::<T, _>(s.len(), |i| s[i].clone())
    }

    /// Allocate a slice of `len` elements in a `Shared`-flavor chunk via `f(i)`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// If `f` panics, already-initialized elements are dropped (drop guard) and the
    /// panic propagates.
    #[inline]
    pub fn alloc_slice_fill_with_arc<T, F>(&self, len: usize, f: F) -> Arc<[T], A>
    where
        T: Send + Sync,
        F: FnMut(usize) -> T,
        A: Send + Sync,
    {
        (self.impl_alloc_slice_arc_with::<T, F>(len, f)).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_slice_fill_with_arc<T, F>(&self, len: usize, f: F) -> Result<Arc<[T], A>, AllocError>
    where
        T: Send + Sync,
        F: FnMut(usize) -> T,
        A: Send + Sync,
    {
        self.impl_alloc_slice_arc_with::<T, F>(len, f)
    }

    /// Allocate a slice in a `Shared`-flavor chunk and fill it from `iter`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// May also panic if the iterator yields fewer elements than its
    /// `ExactSizeIterator::len()` reported.
    #[inline]
    pub fn alloc_slice_fill_iter_arc<T, I>(&self, iter: I) -> Arc<[T], A>
    where
        T: Send + Sync,
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
        A: Send + Sync,
    {
        let it = iter.into_iter();
        let len = it.len();
        (self.impl_alloc_slice_arc_iter::<T, _>(len, it)).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_fill_iter_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_slice_fill_iter_arc<T, I>(&self, iter: I) -> Result<Arc<[T], A>, AllocError>
    where
        T: Send + Sync,
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
        A: Send + Sync,
    {
        let it = iter.into_iter();
        let len = it.len();
        self.impl_alloc_slice_arc_iter::<T, _>(len, it)
    }

    /// Arc + Copy: no element-drop runs, but we still take an Arc-owned
    /// refcount on the chunk.
    #[inline]
    fn impl_alloc_slice_arc_copy<T: Copy>(&self, src: &[T]) -> Result<Arc<[T], A>, AllocError> {
        check_slice_arc_layout::<T>(src.len())?;
        let len = src.len();
        // Copy is never `Drop`, so use the no-drop reservation.
        #[cfg(feature = "stats")]
        let payload_bytes = mem::size_of::<T>().saturating_mul(len);
        let bytes_needed = worst_case_thin_slice_payload::<T>(len);
        loop {
            if let Some((uninit, chunk_ptr)) = self.try_reserve_shared_slice::<T>(len) {
                let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                let slice_ptr = uninit.init_copy_from_slice_ptr(src);
                let _ = chunk_ref.forget();
                #[cfg(feature = "stats")]
                self.record_alloc(payload_bytes);
                // SAFETY: `slice_ptr` points to `len` initialized `T`s in a
                // shared chunk with a fresh +1; `Arc::from_raw` adopts that
                // +1. Chunk-wide provenance preserved via `init_copy_from_slice_ptr`.
                return Ok(unsafe { Arc::from_raw(slice_ptr.cast::<u8>()) });
            }
            if self.is_oversized_shared(bytes_needed) {
                return self.alloc_oversized_shared_with(bytes_needed, |mutator, chunk_ptr| {
                    let ticket = mutator
                        .try_alloc_uninit_slice_prefixed::<T>(len)
                        .expect("dedicated oversized chunk sized to fit slice");
                    let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                    let slice_ptr = ticket.init_copy_from_slice_ptr(src);
                    let _ = chunk_ref.forget();
                    #[cfg(feature = "stats")]
                    self.record_alloc(payload_bytes);
                    // SAFETY: see the non-oversized branch.
                    unsafe { Arc::from_raw(slice_ptr.cast::<u8>()) }
                });
            }
            self.refill_shared(bytes_needed)?;
        }
    }

    /// Arc + closure fill: records a chunk drop entry when `T: Drop`,
    /// so the chunk's teardown runs `T::drop` on each element after the
    /// last `Arc` releases.
    #[inline]
    fn impl_alloc_slice_arc_with<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<Arc<[T], A>, AllocError> {
        check_slice_arc_layout::<T>(len)?;
        #[cfg(feature = "stats")]
        let payload_bytes = mem::size_of::<T>().saturating_mul(len);
        // Refill hint accounts for the length prefix, payload alignment
        // slack, payload bytes, and (for `T: Drop`) a drop-entry slot.
        let bytes_needed = worst_case_thin_slice_payload::<T>(len);
        let mut f = Some(f);
        loop {
            // Branch on needs_drop at const time so monomorphizations
            // pick the right reservation helper.
            if const { mem::needs_drop::<T>() } {
                if let Some((uninit, chunk_ptr)) = self.try_reserve_shared_slice_with_drop::<T>(len) {
                    let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                    let f = f.take().expect("with closure taken twice");
                    let slice_ptr = uninit.init_with_ptr(f);
                    let _ = chunk_ref.forget();
                    #[cfg(feature = "stats")]
                    self.record_alloc(payload_bytes);
                    // SAFETY: see `impl_alloc_slice_arc_copy`; the drop entry
                    // was committed by `init_with_ptr` for the chunk-teardown
                    // path. `slice_ptr` carries chunk-wide provenance so the
                    // Arc's later `byte_sub` to the chunk header is sound.
                    return Ok(unsafe { Arc::from_raw(slice_ptr.cast::<u8>()) });
                }
            } else if let Some((uninit, chunk_ptr)) = self.try_reserve_shared_slice::<T>(len) {
                let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                let f = f.take().expect("with closure taken twice");
                let slice_ptr = uninit.init_with_ptr(f);
                let _ = chunk_ref.forget();
                #[cfg(feature = "stats")]
                self.record_alloc(payload_bytes);
                // SAFETY: see `impl_alloc_slice_arc_copy`; chunk-wide
                // provenance preserved via `init_with_ptr`.
                return Ok(unsafe { Arc::from_raw(slice_ptr.cast::<u8>()) });
            }
            if self.is_oversized_shared(bytes_needed) {
                let fclosure = f.take().expect("with closure taken twice");
                return self.alloc_oversized_shared_with(bytes_needed, |mutator, chunk_ptr| {
                    let slice_ptr = if const { mem::needs_drop::<T>() } {
                        let ticket = mutator
                            .try_alloc_uninit_slice_with_drop_prefixed::<T>(len)
                            .expect("dedicated oversized chunk sized to fit slice + drop entry");
                        let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                        let p = ticket.init_with_ptr(fclosure);
                        let _ = chunk_ref.forget();
                        p
                    } else {
                        let ticket = mutator
                            .try_alloc_uninit_slice_prefixed::<T>(len)
                            .expect("dedicated oversized chunk sized to fit slice");
                        let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
                        let p = ticket.init_with_ptr(fclosure);
                        let _ = chunk_ref.forget();
                        p
                    };
                    #[cfg(feature = "stats")]
                    self.record_alloc(payload_bytes);
                    // SAFETY: see the non-oversized branches above.
                    unsafe { Arc::from_raw(slice_ptr.cast::<u8>()) }
                });
            }
            self.refill_shared(bytes_needed)?;
        }
    }

    #[inline]
    fn impl_alloc_slice_arc_iter<T, I: Iterator<Item = T>>(&self, len: usize, mut iter: I) -> Result<Arc<[T], A>, AllocError> {
        self.impl_alloc_slice_arc_with::<T, _>(len, move |_| {
            iter.next()
                .expect("caller violated ExactSizeIterator contract: iterator yielded fewer elements than reported")
        })
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate `len` slots and fill each via `f(i)`, returning a
    /// [`Pin<Arc<[T], A>>`](core::pin::Pin).
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_with_arc_pin<T, F>(&self, len: usize, f: F) -> Pin<Arc<[T], A>>
    where
        T: Send + Sync,
        F: FnMut(usize) -> T,
        A: Send + Sync + 'static,
    {
        Arc::into_pin(self.alloc_slice_fill_with_arc::<T, F>(len, f))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_arc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_slice_fill_with_arc`].
    #[inline]
    pub fn try_alloc_slice_fill_with_arc_pin<T, F>(&self, len: usize, f: F) -> Result<Pin<Arc<[T], A>>, AllocError>
    where
        T: Send + Sync,
        F: FnMut(usize) -> T,
        A: Send + Sync + 'static,
    {
        self.try_alloc_slice_fill_with_arc::<T, F>(len, f).map(Arc::into_pin)
    }
}

/// Common up-front checks for the `Arc<[T]>` slice family. Rejects
/// over-aligned `T` (would break the smart-pointer header recovery) and
/// `T: Drop` slices whose `len > u16::MAX` (the chunk drop entry packs
/// the element count into a `u16`).
//
// Mutation testing is suppressed here: any mutation that bypasses the
// `len > u16::MAX` rejection (e.g. `&&`→`||`, `>`→`==`) sends the
// caller's refill loop into an unbounded chunk-allocation spin (see the
// detailed note in `alloc_slice_ref::reject_drop_slice_too_long`).
// Correctness is exercised by integration tests in `coverage_gaps.rs`,
// `arena.rs`, and `mutants_extras.rs`.
#[cfg_attr(test, mutants::skip)]
#[inline]
fn check_slice_arc_layout<T>(len: usize) -> Result<(), AllocError> {
    if mem::align_of::<T>() >= MAX_SMART_PTR_ALIGN {
        return Err(AllocError);
    }
    if mem::needs_drop::<T>() && len > u16::MAX as usize {
        return Err(AllocError);
    }
    Ok(())
}
