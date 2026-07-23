// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `&mut [T]` slice allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.
//!
//! # Specialization strategy
//!
//! `alloc_slice_copy` / `try_alloc_slice_copy` and `alloc_slice_clone` /
//! `try_alloc_slice_clone` each dispatch into a shared `#[inline(always)]`
//! helper. The common reservation path stays inline while refill and oversized
//! allocation continuations stay out of line.

use core::hint::assert_unchecked;
use core::mem;
use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::{Arena, ExpectAlloc};
use crate::internal::constants::CHUNK_ALIGN;
use crate::{Alloc, AllocError};

/// Reject over-aligned slice element types early. Simple-reference
/// slices return a plain `&mut [T]` (no header-recovery mask), so they
/// can use the full chunk and only reject alignments that no single
/// chunk could satisfy (`>= CHUNK_ALIGN`). This is a looser cap than the
/// smart-pointer slice paths, which need [`MAX_SMART_PTR_ALIGN`].
#[inline(always)]
fn reject_over_aligned<T>() -> Result<(), AllocError> {
    if const { mem::align_of::<T>() >= CHUNK_ALIGN } {
        return Err(AllocError::ALIGNMENT_TOO_LARGE);
    }
    Ok(())
}

/// Worst-case payload bytes for a slice allocation of `len` `T`s: value
/// bytes + alignment padding. Saturates at `usize::MAX` on overflow — the
/// refill path then fails the allocator on the impossibly large request.
#[cfg_attr(test, mutants::skip)] // under-sized hint ⇒ OOM spin
#[inline]
fn worst_case_slice_payload<T>(len: usize) -> usize {
    let value_bytes = mem::size_of::<T>().saturating_mul(len);
    value_bytes.saturating_add(mem::align_of::<T>())
}

/// Empty `&mut [T]` backed by a well-aligned dangling pointer.
///
/// Used by the `impl_alloc_slice_*` fast paths to bypass the reservation
/// machinery on `len == 0`: a length-0 reservation would otherwise trip
/// the zero-size probe-byte guard in `try_alloc` (which exists to keep
/// smart-pointer value pointers strictly inside the chunk for header
/// recovery). For a plain `&mut [T]` there is no header recovery, and
/// Rust permits a zero-length slice to alias a well-aligned dangling
/// pointer.
#[inline(always)]
fn empty_slice<'a, T>() -> &'a mut [T] {
    // SAFETY: `NonNull::<T>::dangling()` is well-aligned and non-null;
    // an empty `&mut [T]` is well-defined regardless of the pointer
    // value as long as alignment is correct.
    unsafe { core::slice::from_raw_parts_mut(NonNull::<T>::dangling().as_ptr(), 0) }
}

/// Records the caller invariant that `len` is non-zero.
#[inline(always)]
#[cfg_attr(test, mutants::skip)]
fn assume_nonzero_len(len: usize) {
    // SAFETY: callers handle `len == 0` via an early return before this.
    unsafe { assert_unchecked(len > 0) };
}

impl<A: Allocator + Clone> Arena<A> {
    /// Bump-allocate a copy of `slice` (element-by-element `Copy`) into the arena.
    ///
    /// Returns an owning [`Alloc<[T]>`](Alloc) whose lifetime is tied to
    /// `&self`. Like [`Self::alloc`] but for slices of `T: Copy`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_copy`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_copy([1, 2, 3]);
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_slice_copy<T: Copy>(&self, slice: impl AsRef<[T]>) -> Alloc<'_, [T]> {
        self.impl_alloc_slice_copy::<T>(slice.as_ref()).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_copy`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_slice_copy([2, 4]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[2, 4]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_copy<T: Copy>(&self, slice: impl AsRef<[T]>) -> Result<Alloc<'_, [T]>, AllocError> {
        self.impl_alloc_slice_copy::<T>(slice.as_ref())
    }

    /// Bump-allocate a slice and fill it with values pulled from `f`.
    ///
    /// Returns an owning [`Alloc<[T]>`](Alloc) whose lifetime is tied to
    /// `&self`. The slice's elements are dropped when the [`Alloc`] is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_with`] for a fallible variant.
    ///
    /// If `f` panics, already-initialized elements are dropped (drop guard) and the
    /// panic propagates.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_fill_with(3, |i| i * 2);
    /// assert_eq!(&*value, &[0, 2, 4]);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_with<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Alloc<'_, [T]> {
        self.impl_alloc_slice_fill_with::<T, F>(len, f).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// If `f` panics, already-initialized elements are dropped.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_slice_fill_with(3, |i| i + 1) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_fill_with<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<Alloc<'_, [T]>, AllocError> {
        self.impl_alloc_slice_fill_with::<T, F>(len, f)
    }

    /// Bump-allocate a slice by cloning each element of `slice` into the arena.
    ///
    /// Returns an owning [`Alloc<[T]>`](Alloc) whose lifetime is tied to `&self`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_clone`] for a fallible variant.
    ///
    /// May panic if `T::clone` panics; already-cloned elements are dropped before the
    /// panic propagates.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_clone([String::from("a"), String::from("b")]);
    /// assert_eq!(value[1], "b");
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_slice_clone<T: Clone>(&self, slice: impl AsRef<[T]>) -> Alloc<'_, [T]> {
        self.impl_alloc_slice_clone::<T>(slice.as_ref()).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_clone`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// May panic if a `T::clone` impl panics; already-cloned elements
    /// are dropped before the panic propagates.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_slice_clone([String::from("x")]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(value[0], "x");
    /// ```
    #[inline]
    pub fn try_alloc_slice_clone<T: Clone>(&self, slice: impl AsRef<[T]>) -> Result<Alloc<'_, [T]>, AllocError> {
        self.impl_alloc_slice_clone::<T>(slice.as_ref())
    }

    /// Bump-allocate a slice and fill it with values pulled from `iter`.
    ///
    /// Returns an owning [`Alloc<[T]>`](Alloc) whose lifetime is tied to
    /// `&self`. The slice's elements are dropped when the [`Alloc`] is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_iter`] for a fallible variant.
    ///
    /// May also panic if the iterator yields fewer elements than its
    /// `ExactSizeIterator::len()` reported.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_fill_iter([1, 2, 3]);
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_iter<T, I>(&self, iter: I) -> Alloc<'_, [T]>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        self.impl_alloc_slice_fill_iter::<T, I::IntoIter>(iter.into_iter()).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_fill_iter`].
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_slice_fill_iter([3, 2, 1]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[3, 2, 1]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_fill_iter<T, I>(&self, iter: I) -> Result<Alloc<'_, [T]>, AllocError>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        self.impl_alloc_slice_fill_iter::<T, I::IntoIter>(iter.into_iter())
    }

    /// Adopting wrapper over [`Self::alloc_slice_copy_raw`]: writes the slice
    /// into a fresh arena slot and takes ownership of it in an [`Alloc`].
    #[inline(always)]
    fn impl_alloc_slice_copy<T: Copy>(&self, src: &[T]) -> Result<Alloc<'_, [T]>, AllocError> {
        let slot = self.alloc_slice_copy_raw::<T>(src)?;
        // SAFETY: `alloc_slice_copy_raw` returns the unique `&mut [T]` for a
        // freshly-written arena slice that the arena hands out exactly once and
        // never drops itself, so `Alloc` may adopt it and own its destructor.
        Ok(unsafe { Alloc::from_mut(slot) })
    }

    /// Raw allocation path shared by `alloc_slice_copy` and
    /// `try_alloc_slice_copy`. `T: Copy` requires no drop entry.
    #[inline(always)]
    fn alloc_slice_copy_raw<T: Copy>(&self, src: &[T]) -> Result<&mut [T], AllocError> {
        reject_over_aligned::<T>()?;
        let len = src.len();
        if len == 0 {
            return Ok(empty_slice::<T>());
        }
        assume_nonzero_len(len);
        // A live slice has a representable byte size.
        let size = mem::size_of_val(src);
        loop {
            if let Some(u) = self.try_reserve_local_slice_with_size::<T>(len, size) {
                return Ok(u.init_copy_from_slice(src));
            }
            if let Some(slice) = self.refill_or_alloc_oversized_slice_copy::<T>(src)? {
                return Ok(slice);
            }
        }
    }

    /// Refills the current chunk and returns `Ok(None)` for a retry, or
    /// returns a slice allocated from a dedicated oversized chunk.
    #[cold]
    #[inline(never)]
    // Skipped because removing the refill side effect makes the caller's retry
    // loop infinite.
    #[cfg_attr(test, mutants::skip)]
    fn refill_or_alloc_oversized_slice_copy<T: Copy>(&self, src: &[T]) -> Result<Option<&mut [T]>, AllocError> {
        let refill_hint = worst_case_slice_payload::<T>(src.len());
        if self.is_oversized(refill_hint) {
            return Ok(Some(self.alloc_oversized_slice_copy::<T>(refill_hint, src)?));
        }
        self.refill(refill_hint)?;
        Ok(None)
    }

    /// Out-of-line oversized fallback for [`Self::alloc_slice_copy_raw`].
    #[cold]
    #[inline(never)]
    #[expect(
        clippy::mut_from_ref,
        reason = "internal helper hands out a fresh, disjoint arena slot per call; the returned &mut is wrapped in an owning Alloc at the public boundary"
    )]
    fn alloc_oversized_slice_copy<T: Copy>(&self, refill_hint: usize, src: &[T]) -> Result<&mut [T], AllocError> {
        let mutator = self.acquire_oversized_local_mutator(refill_hint)?;
        let ticket = mutator
            .try_alloc_uninit_slice::<T>(src.len())
            .expect("dedicated oversized chunk sized to fit slice");
        let mut ptr = ticket.init_copy_from_slice_ptr(src);
        self.retain_oversized_local_mutator(mutator);
        // SAFETY: chunk retained in `retired_local` for `&self`.
        Ok(unsafe { ptr.as_mut() })
    }

    /// Adopting wrapper over [`Self::alloc_slice_clone_raw`]: clones the slice
    /// into a fresh arena slot and takes ownership of it in an [`Alloc`].
    #[inline(always)]
    fn impl_alloc_slice_clone<T: Clone>(&self, src: &[T]) -> Result<Alloc<'_, [T]>, AllocError> {
        let slot = self.alloc_slice_clone_raw::<T>(src)?;
        // SAFETY: `alloc_slice_clone_raw` returns the unique `&mut [T]` for a
        // freshly-written arena slice that the arena hands out exactly once and
        // never drops itself, so `Alloc` may adopt it and own its destructor.
        Ok(unsafe { Alloc::from_mut(slot) })
    }

    /// Closure-free fast path for `alloc_slice_clone` /
    /// `try_alloc_slice_clone`. `PANIC` monomorphizes the error arm.
    #[inline(always)]
    fn alloc_slice_clone_raw<T: Clone>(&self, src: &[T]) -> Result<&mut [T], AllocError> {
        reject_over_aligned::<T>()?;
        let len = src.len();
        if len == 0 {
            return Ok(empty_slice::<T>());
        }
        // See `alloc_slice_copy_raw`.
        assume_nonzero_len(len);
        // See `alloc_slice_copy_raw`. Hoisted byte size lets the reservation
        // skip the `checked_mul` overflow guard.
        let size = mem::size_of_val(src);
        loop {
            if let Some(u) = self.try_reserve_local_slice_with_size::<T>(len, size) {
                return Ok(u.init_clone_from_slice(src));
            }
            if let Some(slice) = self.refill_or_alloc_oversized_slice_clone::<T>(src)? {
                return Ok(slice);
            }
        }
    }

    /// Cold fall-back for [`Self::alloc_slice_clone_raw`]. See
    /// [`Self::refill_or_alloc_oversized_slice_copy`] for the rationale
    /// behind the split.
    #[cold]
    #[inline(never)]
    #[expect(
        clippy::mut_from_ref,
        reason = "internal helper hands out a fresh, disjoint arena slot per call; the returned &mut is wrapped in an owning Alloc at the public boundary"
    )]
    // Skipped because returning without refilling makes the retry loop infinite.
    #[cfg_attr(test, mutants::skip)]
    fn refill_or_alloc_oversized_slice_clone<T: Clone>(&self, src: &[T]) -> Result<Option<&mut [T]>, AllocError> {
        let len = src.len();
        let refill_hint = worst_case_slice_payload::<T>(len);
        if self.is_oversized(refill_hint) {
            let mut ptr = self.alloc_oversized_local_with(refill_hint, |mutator| {
                let ticket = mutator
                    .try_alloc_uninit_slice::<T>(len)
                    .expect("dedicated oversized chunk sized to fit slice");
                ticket.init_with_ptr(|i| src[i].clone())
            })?;
            // SAFETY: chunk retained in `retired_local` for `&self`.
            return Ok(Some(unsafe { ptr.as_mut() }));
        }
        self.refill(refill_hint)?;
        Ok(None)
    }

    /// Adopting wrapper over [`Self::alloc_slice_fill_with_raw`]: fills a fresh
    /// arena slot from `f` and takes ownership of it in an [`Alloc`].
    #[inline(always)]
    fn impl_alloc_slice_fill_with<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<Alloc<'_, [T]>, AllocError> {
        let slot = self.alloc_slice_fill_with_raw::<T, F>(len, f)?;
        // SAFETY: `alloc_slice_fill_with_raw` returns the unique `&mut [T]` for
        // a freshly-written arena slice that the arena hands out exactly once
        // and never drops itself, so `Alloc` may adopt it and own its
        // destructor.
        Ok(unsafe { Alloc::from_mut(slot) })
    }

    /// Closure-bearing fast path for `alloc_slice_fill_with` /
    /// `try_alloc_slice_fill_with`. Keeping the refill continuation out of
    /// line avoids materializing closure state on the common path.
    #[inline(always)]
    fn alloc_slice_fill_with_raw<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<&mut [T], AllocError> {
        reject_over_aligned::<T>()?;
        if len == 0 {
            return Ok(empty_slice::<T>());
        }
        // See `alloc_slice_copy_raw`.
        assume_nonzero_len(len);
        let refill_hint = worst_case_slice_payload::<T>(len);
        if let Some(u) = self.try_reserve_local_slice::<T>(len) {
            return Ok(u.init_with(f));
        }
        self.alloc_slice_fill_with_refill(len, refill_hint, f)
    }

    /// Cold refill path for [`Self::alloc_slice_fill_with_raw`].
    #[cold]
    #[inline(never)]
    #[expect(
        clippy::mut_from_ref,
        reason = "internal helper hands out a fresh, disjoint arena slot per call; the returned &mut is wrapped in an owning Alloc by impl_alloc_slice_fill_with"
    )]
    // Skipped because removing the refill side effect makes the retry loop
    // infinite.
    #[cfg_attr(test, mutants::skip)]
    fn alloc_slice_fill_with_refill<T, F: FnMut(usize) -> T>(&self, len: usize, refill_hint: usize, f: F) -> Result<&mut [T], AllocError> {
        let mut f = Some(f);
        loop {
            if let Some(u) = self.try_reserve_local_slice::<T>(len) {
                let f = f.take().expect("with closure taken twice");
                return Ok(u.init_with(f));
            }
            if self.is_oversized(refill_hint) {
                let f = f.take().expect("with closure taken twice");
                let mut ptr = self.alloc_oversized_local_with(refill_hint, |mutator| {
                    let ticket = mutator
                        .try_alloc_uninit_slice::<T>(len)
                        .expect("dedicated oversized chunk sized to fit slice");
                    ticket.init_with_ptr(f)
                })?;
                // SAFETY: chunk retained in `retired_local` for `&self`.
                return Ok(unsafe { ptr.as_mut() });
            }
            self.refill(refill_hint)?;
        }
    }

    /// Adopting wrapper over [`Self::alloc_slice_fill_iter_raw`]: fills a fresh
    /// arena slot from `iter` and takes ownership of it in an [`Alloc`].
    #[inline(always)]
    fn impl_alloc_slice_fill_iter<T, I: ExactSizeIterator<Item = T>>(&self, iter: I) -> Result<Alloc<'_, [T]>, AllocError> {
        let slot = self.alloc_slice_fill_iter_raw::<T, I>(iter)?;
        // SAFETY: `alloc_slice_fill_iter_raw` returns the unique `&mut [T]` for
        // a freshly-written arena slice that the arena hands out exactly once
        // and never drops itself, so `Alloc` may adopt it and own its
        // destructor.
        Ok(unsafe { Alloc::from_mut(slot) })
    }

    /// Iterator-bearing fast path for `alloc_slice_fill_iter` /
    /// `try_alloc_slice_fill_iter`. The iterator length is sampled once
    /// via [`ExactSizeIterator::len`] before reservation. Keeping the refill
    /// continuation out of line avoids materializing iterator state on the
    /// common path. The iterator is consumed only on success arms that return.
    #[inline(always)]
    fn alloc_slice_fill_iter_raw<T, I: ExactSizeIterator<Item = T>>(&self, iter: I) -> Result<&mut [T], AllocError> {
        reject_over_aligned::<T>()?;
        let len = iter.len();
        if len == 0 {
            // Drop the iterator without consuming it: the contract is
            // "fill `len` slots from the iterator", so a zero-length
            // fill consumes nothing.
            drop(iter);
            return Ok(empty_slice::<T>());
        }
        // See `alloc_slice_copy_raw`.
        assume_nonzero_len(len);
        let refill_hint = worst_case_slice_payload::<T>(len);
        if let Some(u) = self.try_reserve_local_slice::<T>(len) {
            return Ok(u.init_from_iter(iter));
        }
        self.alloc_slice_fill_iter_refill(len, refill_hint, iter)
    }

    /// Cold refill path for [`Self::alloc_slice_fill_iter_raw`].
    #[cold]
    #[inline(never)]
    #[expect(
        clippy::mut_from_ref,
        reason = "internal helper hands out a fresh, disjoint arena slot per call; the returned &mut is wrapped in an owning Alloc by impl_alloc_slice_fill_iter"
    )]
    // Skipped because removing the refill side effect makes the retry loop
    // infinite.
    #[cfg_attr(test, mutants::skip)]
    fn alloc_slice_fill_iter_refill<T, I: Iterator<Item = T>>(
        &self,
        len: usize,
        refill_hint: usize,
        iter: I,
    ) -> Result<&mut [T], AllocError> {
        loop {
            if let Some(u) = self.try_reserve_local_slice::<T>(len) {
                return Ok(u.init_from_iter(iter));
            }
            if self.is_oversized(refill_hint) {
                let mut ptr = self.alloc_oversized_local_with(refill_hint, |mutator| {
                    let ticket = mutator
                        .try_alloc_uninit_slice::<T>(len)
                        .expect("dedicated oversized chunk sized to fit slice");
                    ticket.init_from_iter_ptr(iter)
                })?;
                // SAFETY: chunk retained in `retired_local` for `&self`.
                return Ok(unsafe { ptr.as_mut() });
            }
            self.refill(refill_hint)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Arena;

    #[test]
    fn fill_iter_raw_uses_existing_chunk() {
        let arena = Arena::new();
        let _prime = arena.alloc(0_u8);

        let slice = arena.alloc_slice_fill_iter_raw([1_u8, 2, 3].into_iter()).unwrap();

        assert_eq!(slice, [1, 2, 3]);
    }
}
