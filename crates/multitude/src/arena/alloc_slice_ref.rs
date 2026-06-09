// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `&mut [T]` slice allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.
//!
//! # Specialization strategy
//!
//! `alloc_slice_copy` / `try_alloc_slice_copy` and `alloc_slice_clone` /
//! `try_alloc_slice_clone` each dispatch into a single
//! `#[inline(always)]` helper parameterized by `const PANIC: bool`,
//! exactly like [`super::alloc_value`] and [`super::alloc_str`]. The
//! clone helper additionally branches on
//! `const { mem::needs_drop::<T>() }` to specialize away the drop-entry
//! reservation for trivial-drop element types.

use core::mem;

use allocator_api2::alloc::{AllocError, Allocator};

use super::{Arena, ExpectAlloc};
use crate::internal::constants::CHUNK_ALIGN;
use crate::internal::drop_entry::DropEntry;

/// Reject over-aligned slice element types early. Simple-reference
/// slices return a plain `&mut [T]` (no header-recovery mask), so they
/// can use the full chunk and only reject alignments that no single
/// chunk could satisfy (`>= CHUNK_ALIGN`). This is a looser cap than the
/// smart-pointer slice paths, which need [`MAX_SMART_PTR_ALIGN`].
#[inline(always)]
fn reject_over_aligned<T>() -> Result<(), AllocError> {
    if const { mem::align_of::<T>() >= CHUNK_ALIGN } {
        return Err(AllocError);
    }
    Ok(())
}

/// Reject `T: Drop` slices whose `len` exceeds `u16::MAX`: the chunk
/// drop entry packs the element count into a `u16`, so a longer
/// drop-tracked slice can never be encoded. Without this up-front
/// rejection the reservation helper returns `None` for every chunk,
/// and the caller's refill loop spins forever, allocating (and
/// retaining) a fresh oversized chunk on each iteration until the
/// process runs out of memory. `T: !Drop` slices need no drop entry
/// and are unbounded.
#[cfg_attr(test, mutants::skip)] // any mutation bypassing the guard ⇒ OOM spin
#[inline]
fn reject_drop_slice_too_long<T>(len: usize) -> Result<(), AllocError> {
    if mem::needs_drop::<T>() && len > u16::MAX as usize {
        return Err(AllocError);
    }
    Ok(())
}

/// Worst-case payload bytes for a slice allocation of `len` `T`s: value
/// bytes + alignment padding, plus one [`DropEntry`] slot when `T`
/// requires drop. Saturates at `usize::MAX` on overflow — the refill
/// path then fails the allocator on the impossibly large request.
#[cfg_attr(test, mutants::skip)] // under-sized hint ⇒ OOM spin
#[inline]
fn worst_case_slice_payload<T>(len: usize) -> usize {
    let value_bytes = mem::size_of::<T>().saturating_mul(len);
    let base = value_bytes.saturating_add(mem::align_of::<T>());
    if mem::needs_drop::<T>() {
        base.saturating_add(mem::size_of::<DropEntry>())
    } else {
        base
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Bump-allocate a copy of `slice` (element-by-element `Copy`) into the arena.
    ///
    /// Returns a mutable slice whose lifetime is tied to `&self`. Like
    /// [`Self::alloc`] but for slices of `T: Copy`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_copy`] for a fallible variant.
    #[must_use]
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn alloc_slice_copy<T: Copy>(&self, slice: impl AsRef<[T]>) -> &mut [T] {
        (self.impl_alloc_slice_copy::<T>(slice.as_ref())).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_copy`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[allow(clippy::mut_from_ref, reason = "simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc_slice_copy<T: Copy>(&self, slice: impl AsRef<[T]>) -> Result<&mut [T], AllocError> {
        self.impl_alloc_slice_copy::<T>(slice.as_ref())
    }

    /// Bump-allocate a slice and fill it with values pulled from `f`.
    ///
    /// Returns a mutable slice whose lifetime is tied to `&self`. If
    /// `T: Drop`, a drop entry is registered (drops at arena drop).
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_with`] for a fallible variant.
    ///
    /// If `f` panics, already-initialized elements are dropped (drop guard) and the
    /// panic propagates.
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_with<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> &mut [T] {
        (self.impl_alloc_slice_fill_with::<T, F>(len, f)).expect_alloc()
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
    #[allow(clippy::mut_from_ref, reason = "simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc_slice_fill_with<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<&mut [T], AllocError> {
        self.impl_alloc_slice_fill_with::<T, F>(len, f)
    }

    /// Bump-allocate a slice by cloning each element of `slice` into the arena.
    ///
    /// Returns a mutable slice whose lifetime is tied to `&self`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_clone`] for a fallible variant.
    ///
    /// May panic if `T::clone` panics; already-cloned elements are dropped before the
    /// panic propagates.
    #[must_use]
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn alloc_slice_clone<T: Clone>(&self, slice: impl AsRef<[T]>) -> &mut [T] {
        (self.impl_alloc_slice_clone::<T>(slice.as_ref())).expect_alloc()
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
    #[allow(clippy::mut_from_ref, reason = "simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc_slice_clone<T: Clone>(&self, slice: impl AsRef<[T]>) -> Result<&mut [T], AllocError> {
        self.impl_alloc_slice_clone::<T>(slice.as_ref())
    }

    /// Bump-allocate a slice and fill it with values pulled from `iter`.
    ///
    /// Returns a mutable slice whose lifetime is tied to `&self`. If
    /// `T: Drop`, a drop entry is registered (drops at arena drop).
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_fill_iter`] for a fallible variant.
    ///
    /// May also panic if the iterator yields fewer elements than its
    /// `ExactSizeIterator::len()` reported.
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_iter<T, I>(&self, iter: I) -> &mut [T]
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        (self.impl_alloc_slice_fill_iter::<T, I::IntoIter>(iter.into_iter())).expect_alloc()
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
    #[inline]
    #[allow(clippy::mut_from_ref, reason = "see `try_alloc_with`")]
    pub fn try_alloc_slice_fill_iter<T, I>(&self, iter: I) -> Result<&mut [T], AllocError>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        self.impl_alloc_slice_fill_iter::<T, I::IntoIter>(iter.into_iter())
    }

    /// Closure-free fast path for `alloc_slice_copy` / `try_alloc_slice_copy`.
    /// Because `T: Copy` implies `!Drop`, this never reserves a drop
    /// entry; the body monomorphizes to a single bump + memcpy + retry
    /// loop with the `PANIC` arm folded to either `panic_alloc!()` or `?`.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline(always)]
    fn impl_alloc_slice_copy<T: Copy>(&self, src: &[T]) -> Result<&mut [T], AllocError> {
        reject_over_aligned::<T>()?;
        let len = src.len();
        let refill_hint = worst_case_slice_payload::<T>(len);
        loop {
            if let Some(u) = self.try_reserve_local_slice::<T>(len) {
                #[cfg(feature = "stats")]
                self.record_alloc(mem::size_of_val(src));
                return Ok(u.init_copy_from_slice(src));
            }
            if self.is_oversized_local(refill_hint) {
                let mut ptr = self.alloc_oversized_local_with(refill_hint, |mutator| {
                    let ticket = mutator
                        .try_alloc_uninit_slice::<T>(len)
                        .expect("dedicated oversized chunk sized to fit slice");
                    #[cfg(feature = "stats")]
                    self.record_alloc(mem::size_of_val(src));
                    ticket.init_copy_from_slice_ptr(src)
                })?;
                // SAFETY: chunk retained in `retired_local` for `&self`.
                return Ok(unsafe { ptr.as_mut() });
            }
            self.refill_local(refill_hint)?;
        }
    }

    /// Closure-free fast path for `alloc_slice_clone` /
    /// `try_alloc_slice_clone`. Mirrors `impl_alloc_value`: a
    /// `const { mem::needs_drop::<T>() }` branch picks the
    /// drop-entry-bearing reservation for `T: Drop`, and `PANIC`
    /// monomorphizes the error arm.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline(always)]
    fn impl_alloc_slice_clone<T: Clone>(&self, src: &[T]) -> Result<&mut [T], AllocError> {
        reject_over_aligned::<T>()?;
        reject_drop_slice_too_long::<T>(src.len())?;
        let len = src.len();
        let refill_hint = worst_case_slice_payload::<T>(len);
        loop {
            if const { mem::needs_drop::<T>() } {
                if let Some(u) = self.try_reserve_local_slice_with_drop::<T>(len) {
                    #[cfg(feature = "stats")]
                    self.record_alloc(mem::size_of_val(src));
                    return Ok(u.init_clone_from_slice(src));
                }
            } else if let Some(u) = self.try_reserve_local_slice::<T>(len) {
                #[cfg(feature = "stats")]
                self.record_alloc(mem::size_of_val(src));
                return Ok(u.init_clone_from_slice(src));
            }
            if self.is_oversized_local(refill_hint) {
                let mut ptr = self.alloc_oversized_local_with(refill_hint, |mutator| {
                    #[cfg(feature = "stats")]
                    self.record_alloc(mem::size_of_val(src));
                    if const { mem::needs_drop::<T>() } {
                        let ticket = mutator
                            .try_alloc_uninit_slice_with_drop::<T>(len)
                            .expect("dedicated oversized chunk sized to fit slice + drop entry");
                        ticket.init_with_ptr(|i| src[i].clone())
                    } else {
                        let ticket = mutator
                            .try_alloc_uninit_slice::<T>(len)
                            .expect("dedicated oversized chunk sized to fit slice");
                        ticket.init_with_ptr(|i| src[i].clone())
                    }
                })?;
                // SAFETY: chunk retained in `retired_local` for `&self`.
                return Ok(unsafe { ptr.as_mut() });
            }
            self.refill_local(refill_hint)?;
        }
    }

    /// Closure-bearing fast path for `alloc_slice_fill_with` /
    /// `try_alloc_slice_fill_with`. Mirrors `impl_alloc_slice_clone`: a
    /// `const { mem::needs_drop::<T>() }` branch picks the
    /// drop-entry-bearing reservation for `T: Drop`, and `PANIC`
    /// monomorphizes the error arm. `f` is only invoked on the success
    /// arms that `return`, so it stays live across the refill loop
    /// without an `Option<F>` wrapper.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline(always)]
    fn impl_alloc_slice_fill_with<T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<&mut [T], AllocError> {
        reject_over_aligned::<T>()?;
        reject_drop_slice_too_long::<T>(len)?;
        let refill_hint = worst_case_slice_payload::<T>(len);
        let mut f = Some(f);
        loop {
            if const { mem::needs_drop::<T>() } {
                if let Some(u) = self.try_reserve_local_slice_with_drop::<T>(len) {
                    let f = f.take().expect("with closure taken twice");
                    #[cfg(feature = "stats")]
                    self.record_alloc(mem::size_of::<T>() * len);
                    return Ok(u.init_with(f));
                }
            } else if let Some(u) = self.try_reserve_local_slice::<T>(len) {
                let f = f.take().expect("with closure taken twice");
                #[cfg(feature = "stats")]
                self.record_alloc(mem::size_of::<T>() * len);
                return Ok(u.init_with(f));
            }
            if self.is_oversized_local(refill_hint) {
                let f = f.take().expect("with closure taken twice");
                let mut ptr = self.alloc_oversized_local_with(refill_hint, |mutator| {
                    #[cfg(feature = "stats")]
                    self.record_alloc(mem::size_of::<T>() * len);
                    if const { mem::needs_drop::<T>() } {
                        let ticket = mutator
                            .try_alloc_uninit_slice_with_drop::<T>(len)
                            .expect("dedicated oversized chunk sized to fit slice + drop entry");
                        ticket.init_with_ptr(f)
                    } else {
                        let ticket = mutator
                            .try_alloc_uninit_slice::<T>(len)
                            .expect("dedicated oversized chunk sized to fit slice");
                        ticket.init_with_ptr(f)
                    }
                })?;
                // SAFETY: chunk retained in `retired_local` for `&self`.
                return Ok(unsafe { ptr.as_mut() });
            }
            self.refill_local(refill_hint)?;
        }
    }

    /// Iterator-bearing fast path for `alloc_slice_fill_iter` /
    /// `try_alloc_slice_fill_iter`. The iterator length is sampled once
    /// via [`ExactSizeIterator::len`] before reservation; the same
    /// `const PANIC` / `const needs_drop` monomorphization pattern
    /// applies as in [`Self::impl_alloc_slice_fill_with`]. The iterator
    /// is consumed only on the success arms that `return`.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline(always)]
    fn impl_alloc_slice_fill_iter<T, I: ExactSizeIterator<Item = T>>(&self, iter: I) -> Result<&mut [T], AllocError> {
        reject_over_aligned::<T>()?;
        let len = iter.len();
        reject_drop_slice_too_long::<T>(len)?;
        let refill_hint = worst_case_slice_payload::<T>(len);
        let mut iter = Some(iter);
        loop {
            if const { mem::needs_drop::<T>() } {
                if let Some(u) = self.try_reserve_local_slice_with_drop::<T>(len) {
                    let it = iter.take().expect("iterator taken twice");
                    #[cfg(feature = "stats")]
                    self.record_alloc(mem::size_of::<T>() * len);
                    return Ok(u.init_from_iter(it));
                }
            } else if let Some(u) = self.try_reserve_local_slice::<T>(len) {
                let it = iter.take().expect("iterator taken twice");
                #[cfg(feature = "stats")]
                self.record_alloc(mem::size_of::<T>() * len);
                return Ok(u.init_from_iter(it));
            }
            if self.is_oversized_local(refill_hint) {
                let mut it = iter.take().expect("iterator taken twice");
                let mut ptr = self.alloc_oversized_local_with(refill_hint, |mutator| {
                    #[cfg(feature = "stats")]
                    self.record_alloc(mem::size_of::<T>() * len);
                    if const { mem::needs_drop::<T>() } {
                        let ticket = mutator
                            .try_alloc_uninit_slice_with_drop::<T>(len)
                            .expect("dedicated oversized chunk sized to fit slice + drop entry");
                        ticket.init_with_ptr(|_| it.next().expect("ExactSizeIterator yielded fewer elements than reported"))
                    } else {
                        let ticket = mutator
                            .try_alloc_uninit_slice::<T>(len)
                            .expect("dedicated oversized chunk sized to fit slice");
                        ticket.init_from_iter_ptr(it)
                    }
                })?;
                // SAFETY: chunk retained in `retired_local` for `&self`.
                return Ok(unsafe { ptr.as_mut() });
            }
            self.refill_local(refill_hint)?;
        }
    }
}
