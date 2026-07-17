// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Arc<[T]>` / `Rc<[T]>` slice allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.

use core::mem;
use core::pin::Pin;
use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::alloc_prefixed::worst_case_strong_slice_payload;
use super::alloc_value::{MAX_SMART_PTR_ALIGN, acquire_chunk_ref};
use super::{Arena, ExpectAlloc};
use crate::AllocError;
use crate::arc::Arc;
use crate::internal::thin_dst::{AtomicStrong, LocalStrong, Strong};
use crate::rc::Rc;

impl<A: Allocator + Clone> Arena<A> {
    /// Copy `slice` into a chunk and return an [`Arc`].
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_copy_arc`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_copy_arc([1, 2, 3]);
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[inline]
    pub fn alloc_slice_copy_arc<T: Copy + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Arc<[T], A>
    where
        A: Send + Sync,
    {
        self.try_alloc_slice_copy_arc::<T>(slice).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_copy_arc`].
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
    /// let Ok(value) = arena.try_alloc_slice_copy_arc([1, 2, 3]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_copy_arc<T: Copy + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Result<Arc<[T], A>, AllocError>
    where
        A: Send + Sync,
    {
        self.impl_alloc_slice_smart_copy::<AtomicStrong, T>(slice.as_ref())
    }

    /// Clone every element of `slice` into a chunk and return an [`Arc`].
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// May panic if `T::clone` panics; already-cloned elements are dropped before the
    /// panic propagates.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_clone_arc([String::from("a"), String::from("b")]);
    /// assert_eq!(&*value, &[String::from("a"), String::from("b")]);
    /// ```
    #[inline]
    pub fn alloc_slice_clone_arc<T: Clone + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Arc<[T], A>
    where
        A: Send + Sync,
    {
        self.try_alloc_slice_clone_arc::<T>(slice).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_clone_arc`].
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
    /// let Ok(value) = arena.try_alloc_slice_clone_arc([String::from("a"), String::from("b")]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[String::from("a"), String::from("b")]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_clone_arc<T: Clone + Send + Sync>(&self, slice: impl AsRef<[T]>) -> Result<Arc<[T], A>, AllocError>
    where
        A: Send + Sync,
    {
        let s = slice.as_ref();
        self.impl_alloc_slice_smart_with::<AtomicStrong, T, _>(s.len(), |i| s[i].clone())
    }

    /// Allocate a slice of `len` elements in a chunk via `f(i)`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// If `f` panics, already-initialized elements are dropped (drop guard) and the
    /// panic propagates.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_fill_with_arc(3, |i| i + 1);
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[inline]
    pub fn alloc_slice_fill_with_arc<T, F>(&self, len: usize, f: F) -> Arc<[T], A>
    where
        T: Send + Sync,
        F: FnMut(usize) -> T,
        A: Send + Sync,
    {
        self.try_alloc_slice_fill_with_arc::<T, F>(len, f).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_arc`].
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
    /// let Ok(value) = arena.try_alloc_slice_fill_with_arc(3, |i| i + 1) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_fill_with_arc<T, F>(&self, len: usize, f: F) -> Result<Arc<[T], A>, AllocError>
    where
        T: Send + Sync,
        F: FnMut(usize) -> T,
        A: Send + Sync,
    {
        self.impl_alloc_slice_smart_with::<AtomicStrong, T, F>(len, f)
    }

    /// Allocate a slice in a chunk and fill it from `iter`.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// May also panic if the iterator yields fewer elements than its
    /// `ExactSizeIterator::len()` reported.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_fill_iter_arc([3, 1, 4]);
    /// assert_eq!(&*value, &[3, 1, 4]);
    /// ```
    #[inline]
    pub fn alloc_slice_fill_iter_arc<T, I>(&self, iter: I) -> Arc<[T], A>
    where
        T: Send + Sync,
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
        A: Send + Sync,
    {
        self.try_alloc_slice_fill_iter_arc::<T, I>(iter).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_fill_iter_arc`].
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
    /// let Ok(value) = arena.try_alloc_slice_fill_iter_arc([3, 1, 4]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[3, 1, 4]);
    /// ```
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
        self.impl_alloc_slice_smart_iter::<AtomicStrong, T, _>(len, it)
    }

    // ===== `Rc<[T]>` mirror (non-atomic, no Send/Sync bound) =====

    /// Copy `slice` into a chunk and return an [`Rc`].
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_slice_copy_rc`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_copy_rc([1, 2, 3]);
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[inline]
    pub fn alloc_slice_copy_rc<T: Copy>(&self, slice: impl AsRef<[T]>) -> Rc<[T], A> {
        self.try_alloc_slice_copy_rc::<T>(slice).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_copy_rc`].
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
    /// let Ok(value) = arena.try_alloc_slice_copy_rc([1, 2, 3]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_copy_rc<T: Copy>(&self, slice: impl AsRef<[T]>) -> Result<Rc<[T], A>, AllocError> {
        self.impl_alloc_slice_smart_copy::<LocalStrong, T>(slice.as_ref())
    }

    /// Clone every element of `slice` into a chunk and return an [`Rc`].
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// May panic if `T::clone` panics; already-cloned elements are dropped first.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_clone_rc([String::from("a"), String::from("b")]);
    /// assert_eq!(&*value, &[String::from("a"), String::from("b")]);
    /// ```
    #[inline]
    pub fn alloc_slice_clone_rc<T: Clone>(&self, slice: impl AsRef<[T]>) -> Rc<[T], A> {
        self.try_alloc_slice_clone_rc::<T>(slice).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_clone_rc`].
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
    /// let Ok(value) = arena.try_alloc_slice_clone_rc([String::from("a"), String::from("b")]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[String::from("a"), String::from("b")]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_clone_rc<T: Clone>(&self, slice: impl AsRef<[T]>) -> Result<Rc<[T], A>, AllocError> {
        let s = slice.as_ref();
        self.impl_alloc_slice_smart_with::<LocalStrong, T, _>(s.len(), |i| s[i].clone())
    }

    /// Allocate an [`Rc`] slice of `len` elements initialized by `f(i)`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// If `f` panics, already-initialized elements are dropped.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_fill_with_rc(3, |i| i + 1);
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[inline]
    pub fn alloc_slice_fill_with_rc<T, F>(&self, len: usize, f: F) -> Rc<[T], A>
    where
        F: FnMut(usize) -> T,
    {
        self.try_alloc_slice_fill_with_rc::<T, F>(len, f).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_rc`].
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
    /// let Ok(value) = arena.try_alloc_slice_fill_with_rc(3, |i| i + 1) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_fill_with_rc<T, F>(&self, len: usize, f: F) -> Result<Rc<[T], A>, AllocError>
    where
        F: FnMut(usize) -> T,
    {
        self.impl_alloc_slice_smart_with::<LocalStrong, T, F>(len, f)
    }

    /// Allocate a slice in a chunk and fill it from `iter`, returning an [`Rc`].
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// May also panic if the iterator yields fewer elements than reported.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_fill_iter_rc([3, 1, 4]);
    /// assert_eq!(&*value, &[3, 1, 4]);
    /// ```
    #[inline]
    pub fn alloc_slice_fill_iter_rc<T, I>(&self, iter: I) -> Rc<[T], A>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        self.try_alloc_slice_fill_iter_rc::<T, I>(iter).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_slice_fill_iter_rc`].
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
    /// let Ok(value) = arena.try_alloc_slice_fill_iter_rc([3, 1, 4]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[3, 1, 4]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_fill_iter_rc<T, I>(&self, iter: I) -> Result<Rc<[T], A>, AllocError>
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let it = iter.into_iter();
        let len = it.len();
        self.impl_alloc_slice_smart_iter::<LocalStrong, T, _>(len, it)
    }

    // ===== shared generic implementations (return a thin payload pointer) =====

    /// Copy path: no element-drop runs, but we take the family's chunk
    /// refcount and reserve the strong-count prefix. Adopts the finished
    /// payload into `S`'s smart pointer ([`Arc`] or [`Rc`]).
    #[inline]
    fn impl_alloc_slice_smart_copy<S: Strong, T: Copy>(&self, src: &[T]) -> Result<S::Ptr<[T], A>, AllocError> {
        let thin = self.alloc_slice_smart_copy_raw::<S, T>(src)?;
        // SAFETY: `alloc_slice_smart_copy_raw` returns a thin pointer to a
        // fully-initialized `[T]` whose chunk prefix holds a strong count of 1
        // and whose hosting chunk it took a `+1` on; the pointer lies in the
        // chunk's first tile. That is exactly `S::adopt`'s contract.
        Ok(unsafe { S::adopt::<[T], A>(thin) })
    }

    /// Raw copy path returning the thin payload pointer (before adoption into a
    /// smart pointer). Split out so the single `S::adopt` lives in
    /// [`Self::impl_alloc_slice_smart_copy`].
    #[inline]
    fn alloc_slice_smart_copy_raw<S: Strong, T: Copy>(&self, src: &[T]) -> Result<NonNull<u8>, AllocError> {
        check_slice_arc_layout::<T>()?;
        let len = src.len();
        // `src` is a live `&[T]`, so `size_of_val(src)` is a valid `usize`.
        let payload_bytes = mem::size_of_val(src);
        // SAFETY: `payload_bytes == size_of_val(src) == size_of::<T>() * len`,
        // the exact byte count `try_reserve_arc_slice_with_size` requires.
        if let Some((uninit, chunk_ptr)) = unsafe { self.try_reserve_arc_slice_with_size::<S, T>(len, payload_bytes) } {
            let chunk_ref = self.acquire_current_chunk_ref(chunk_ptr);
            let slice_ptr = uninit.init_copy_from_slice_ptr(src);
            let _ = chunk_ref.forget();
            return Ok(slice_ptr.cast::<u8>());
        }
        self.alloc_slice_smart_copy_refill::<S, T>(src, len, payload_bytes)
    }

    /// Cold continuation of [`Self::impl_alloc_slice_smart_copy`]: compute the
    /// worst-case refill hint, then refill (or fall back to a dedicated
    /// oversized chunk) and retry until the reservation succeeds or fails.
    #[cold]
    #[inline(never)]
    fn alloc_slice_smart_copy_refill<S: Strong, T: Copy>(
        &self,
        src: &[T],
        len: usize,
        payload_bytes: usize,
    ) -> Result<NonNull<u8>, AllocError> {
        let bytes_needed = worst_case_strong_slice_payload::<S, T>(len);
        loop {
            // SAFETY: `payload_bytes == size_of_val(src) == size_of::<T>() * len`.
            let reserved = unsafe { self.try_reserve_arc_slice_with_size::<S, T>(len, payload_bytes) };
            if let Some((uninit, chunk_ptr)) = reserved {
                let chunk_ref = self.acquire_current_chunk_ref(chunk_ptr);
                let slice_ptr = uninit.init_copy_from_slice_ptr(src);
                let _ = chunk_ref.forget();
                return Ok(slice_ptr.cast::<u8>());
            }
            if self.is_oversized(bytes_needed) {
                return self.alloc_oversized_shared_with(bytes_needed, |mutator, chunk_ptr| {
                    let (ticket, _chunk) = mutator
                        .try_alloc_arc_slice::<S, T>(len)
                        .expect("dedicated oversized chunk sized to fit slice + strong prefix");
                    let chunk_ref = acquire_chunk_ref::<A>(chunk_ptr);
                    let slice_ptr = ticket.init_copy_from_slice_ptr(src);
                    let _ = chunk_ref.forget();
                    slice_ptr.cast::<u8>()
                });
            }
            self.refill(bytes_needed)?;
        }
    }

    /// Closure-fill path: `T::drop` (if any) runs eagerly via
    /// `drop_in_place::<[T]>` on the smart pointer's last reference. Adopts the
    /// finished payload into `S`'s smart pointer ([`Arc`] or [`Rc`]).
    #[inline]
    fn impl_alloc_slice_smart_with<S: Strong, T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<S::Ptr<[T], A>, AllocError> {
        let thin = self.alloc_slice_smart_with_raw::<S, T, F>(len, f)?;
        // SAFETY: `alloc_slice_smart_with_raw` returns a thin pointer to a
        // fully-initialized `[T]` whose chunk prefix holds a strong count of 1
        // and whose hosting chunk it took a `+1` on; the pointer lies in the
        // chunk's first tile. That is exactly `S::adopt`'s contract.
        Ok(unsafe { S::adopt::<[T], A>(thin) })
    }

    /// Raw closure-fill path returning the thin payload pointer (before
    /// adoption). Split out so the single `S::adopt` lives in
    /// [`Self::impl_alloc_slice_smart_with`].
    #[inline]
    fn alloc_slice_smart_with_raw<S: Strong, T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<NonNull<u8>, AllocError> {
        check_slice_arc_layout::<T>()?;
        if let Some((uninit, chunk_ptr)) = self.try_reserve_arc_slice::<S, T>(len) {
            let chunk_ref = self.acquire_current_chunk_ref(chunk_ptr);
            let slice_ptr = uninit.init_with_ptr(f);
            let _ = chunk_ref.forget();
            return Ok(slice_ptr.cast::<u8>());
        }
        self.alloc_slice_smart_with_refill::<S, T, F>(len, f)
    }

    /// Cold continuation of [`Self::impl_alloc_slice_smart_with`]: compute the
    /// worst-case refill hint, then refill (or fall back to a dedicated
    /// oversized chunk) and retry until the reservation succeeds or fails.
    #[cold]
    #[inline(never)]
    fn alloc_slice_smart_with_refill<S: Strong, T, F: FnMut(usize) -> T>(&self, len: usize, f: F) -> Result<NonNull<u8>, AllocError> {
        let bytes_needed = worst_case_strong_slice_payload::<S, T>(len);
        let mut f = Some(f);
        loop {
            if let Some((uninit, chunk_ptr)) = self.try_reserve_arc_slice::<S, T>(len) {
                let chunk_ref = self.acquire_current_chunk_ref(chunk_ptr);
                let f = f.take().expect("with closure taken twice");
                let slice_ptr = uninit.init_with_ptr(f);
                let _ = chunk_ref.forget();
                return Ok(slice_ptr.cast::<u8>());
            }
            if self.is_oversized(bytes_needed) {
                let fclosure = f.take().expect("with closure taken twice");
                return self.alloc_oversized_shared_with(bytes_needed, |mutator, chunk_ptr| {
                    let (ticket, _chunk) = mutator
                        .try_alloc_arc_slice::<S, T>(len)
                        .expect("dedicated oversized chunk sized to fit slice + strong prefix");
                    let chunk_ref = acquire_chunk_ref::<A>(chunk_ptr);
                    let slice_ptr = ticket.init_with_ptr(fclosure);
                    let _ = chunk_ref.forget();
                    slice_ptr.cast::<u8>()
                });
            }
            self.refill(bytes_needed)?;
        }
    }

    #[inline]
    fn impl_alloc_slice_smart_iter<S: Strong, T, I: Iterator<Item = T>>(
        &self,
        len: usize,
        mut iter: I,
    ) -> Result<S::Ptr<[T], A>, AllocError> {
        self.impl_alloc_slice_smart_with::<S, T, _>(len, move |_| {
            iter.next()
                .expect("caller violated ExactSizeIterator contract: iterator yielded fewer elements than reported")
        })
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate a pinned [`Arc`] slice of `len` elements initialized by `f(i)`.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_fill_with_arc_pin(3, |i| i + 1);
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_slice_fill_with_arc_pin(3, |i| i + 1) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_fill_with_arc_pin<T, F>(&self, len: usize, f: F) -> Result<Pin<Arc<[T], A>>, AllocError>
    where
        T: Send + Sync,
        F: FnMut(usize) -> T,
        A: Send + Sync + 'static,
    {
        self.try_alloc_slice_fill_with_arc::<T, F>(len, f).map(Arc::into_pin)
    }

    /// Allocate a pinned [`Rc`] slice of `len` elements initialized by `f(i)`.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_slice_fill_with_rc_pin(3, |i| i + 1);
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_slice_fill_with_rc_pin<T, F>(&self, len: usize, f: F) -> Pin<Rc<[T], A>>
    where
        F: FnMut(usize) -> T,
        A: 'static,
    {
        Rc::into_pin(self.alloc_slice_fill_with_rc::<T, F>(len, f))
    }

    /// Fallible variant of [`Self::alloc_slice_fill_with_rc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_slice_fill_with_rc`].
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_slice_fill_with_rc_pin(3, |i| i + 1) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &[1, 2, 3]);
    /// ```
    #[inline]
    pub fn try_alloc_slice_fill_with_rc_pin<T, F>(&self, len: usize, f: F) -> Result<Pin<Rc<[T], A>>, AllocError>
    where
        F: FnMut(usize) -> T,
        A: 'static,
    {
        self.try_alloc_slice_fill_with_rc::<T, F>(len, f).map(Rc::into_pin)
    }
}

/// Up-front check for the `Arc<[T]>` / `Rc<[T]>` slice family. Rejects
/// over-aligned `T` (would break the smart-pointer header recovery).
#[inline]
fn check_slice_arc_layout<T>() -> Result<(), AllocError> {
    if mem::align_of::<T>() >= MAX_SMART_PTR_ALIGN {
        return Err(AllocError::ALIGNMENT_TOO_LARGE);
    }
    Ok(())
}
