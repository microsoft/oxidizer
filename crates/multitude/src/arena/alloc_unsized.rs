// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! DST (unsized) value allocation API on [`Arena`].
//!
//! Implements the `alloc_dst_arc`, `alloc_dst_rc`, and `alloc_dst_box`
//! families under the `dst` Cargo feature. Slice lengths are stored in the
//! chunk prefix, while trait-object vtable pointers are stored in each
//! smart-pointer handle. `Arc` and `Rc` run `T`'s destructor eagerly on the
//! last clone via `drop_in_place::<T>`; `Box` does so in its own `Drop`.

use core::alloc::Layout;
use core::pin::Pin;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::Allocator;

use super::alloc_value::acquire_chunk_ref;
use super::{Arena, ExpectAlloc};
use crate::arc::Arc;
use crate::r#box::Box;
use crate::internal::thin_dst::{AtomicStrong, LocalStrong, Strong, strong_prefix_bytes_for};
use crate::rc::Rc;
use crate::{AllocError, SmartPointerPointee};

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate a possibly-unsized `T` and return an `Arc<T, A>`.
    ///
    /// The closure `init` receives a typed fat pointer to the buffer
    /// (built from `(thin_ptr, metadata)`) and is responsible for
    /// writing a valid `T` through it. `T`'s destructor runs eagerly (via
    /// `drop_in_place::<T>`) when the last `Arc` clone is dropped.
    ///
    /// For sized `T`, prefer [`Self::alloc_arc`] / [`Self::alloc_arc_with`].
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails, if `layout.align()` is at least
    /// 32 KiB, or if `init` panics.
    ///
    /// If `init` panics after initializing resources, this function does not
    /// run destructors for those partially initialized contents.
    ///
    /// # Safety
    ///
    /// - `layout` must exactly describe the size and alignment of the
    ///   constructed DST value (e.g., for `[U]` of length `n`,
    ///   `Layout::array::<U>(n).unwrap()`). Passing a smaller layout
    ///   would cause `init` to write past the reservation.
    /// - If `init` returns normally, it must have fully initialized a valid `T`
    ///   at the supplied pointer. Padding bytes need not be initialized.
    /// - `metadata` must describe that value and agree with `layout`.
    /// - `T::Metadata` must use one of Multitude's supported policies: `()` for
    ///   sized values, `usize` for slice-like DSTs, or
    ///   [`ptr_meta::DynMetadata`] for trait-object DSTs.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let value = unsafe {
    ///     arena.alloc_dst_arc::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn alloc_dst_arc<T: ?Sized + Send + Sync + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Arc<T, A>
    where
        A: Send + Sync,
    {
        // SAFETY: forwarded — caller's contract on `layout`/`metadata`/`init`.
        unsafe { self.impl_alloc_dst_arc::<T>(layout, metadata, init) }.expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_dst_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `layout.align()` is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// Panics from the allocator or `init` propagate. If `init` panics after
    /// initializing resources, this function does not run destructors for
    /// those partially initialized contents.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let Ok(value) = (unsafe {
    ///     arena.try_alloc_dst_arc::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// }) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_arc<T: ?Sized + Send + Sync + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Arc<T, A>, AllocError>
    where
        A: Send + Sync,
    {
        // SAFETY: forwarded.
        unsafe { self.impl_alloc_dst_arc::<T>(layout, metadata, init) }
    }

    /// Allocate a possibly-unsized `T` and return a [`Box<T, A>`](crate::Box).
    /// See [`Self::alloc_dst_arc`] for the contract.
    ///
    /// The resulting [`Box`](crate::Box) is the sole owner, so it runs
    /// `T`'s destructor when it is dropped (the `Arc` variants run it
    /// when the last clone is dropped; both are eager).
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// See [`Self::alloc_dst_arc`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let value = unsafe {
    ///     arena.alloc_dst_box::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn alloc_dst_box<T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Box<T, A> {
        // SAFETY: forwarded.
        unsafe { self.impl_alloc_dst_box::<T>(layout, metadata, init) }.expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_dst_box`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_arc`].
    ///
    /// # Panics
    ///
    /// Panics from the allocator or `init` propagate. If `init` panics after
    /// initializing resources, this function does not run destructors for
    /// those partially initialized contents.
    ///
    /// # Safety
    ///
    /// See [`Self::alloc_dst_arc`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let Ok(value) = (unsafe {
    ///     arena.try_alloc_dst_box::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// }) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_box<T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Box<T, A>, AllocError> {
        // SAFETY: forwarded.
        unsafe { self.impl_alloc_dst_box::<T>(layout, metadata, init) }
    }

    /// Allocate a possibly-unsized `T` in an [`Rc<T, A>`](crate::Rc).
    ///
    /// This is the non-atomic, single-thread sibling of [`Self::alloc_dst_arc`]. `T`
    /// needs no `Send`/`Sync` bound.
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let value = unsafe {
    ///     arena.alloc_dst_rc::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn alloc_dst_rc<T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Rc<T, A> {
        // SAFETY: forwarded.
        unsafe { self.impl_alloc_dst_rc::<T>(layout, metadata, init) }.expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_dst_rc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `layout.align()` is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// Panics from the allocator or `init` propagate. If `init` panics after
    /// initializing resources, this function does not run destructors for
    /// those partially initialized contents.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let Ok(value) = (unsafe {
    ///     arena.try_alloc_dst_rc::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// }) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_rc<T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Rc<T, A>, AllocError> {
        // SAFETY: forwarded.
        unsafe { self.impl_alloc_dst_rc::<T>(layout, metadata, init) }
    }

    /// Shared implementation for `alloc_dst_arc` / `try_alloc_dst_arc`.
    ///
    /// Reserves a strong-prefixed shared slot, invokes `init` on the
    /// typed fat pointer, and wraps the result in an [`Arc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    #[inline]
    unsafe fn impl_alloc_dst_arc<T: ?Sized + Send + Sync + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Arc<T, A>, AllocError>
    where
        A: Send + Sync,
    {
        // SAFETY: forwarded to `impl_alloc_dst_smart`, which shares this
        // method's `layout` / `metadata` / `init` contract and returns the
        // adopted `Arc`.
        unsafe { self.impl_alloc_dst_smart::<AtomicStrong, T>(layout, metadata, init) }
    }

    /// `Rc` mirror of [`Self::impl_alloc_dst_arc`] (non-atomic strong count).
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_rc`].
    #[inline]
    unsafe fn impl_alloc_dst_rc<T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Rc<T, A>, AllocError> {
        // SAFETY: forwarded to `impl_alloc_dst_smart`, which shares this
        // method's `layout` / `metadata` / `init` contract and returns the
        // adopted `Rc`.
        unsafe { self.impl_alloc_dst_smart::<LocalStrong, T>(layout, metadata, init) }
    }

    /// Shared implementation for `alloc_dst_box` / `try_alloc_dst_box`.
    /// Like `impl_alloc_dst_arc` but without the shared strong-count
    /// prefix: [`Box::drop`] runs `drop_in_place::<T>` on the value
    /// pointer (which natively handles `?Sized`).
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_box`].
    #[inline]
    unsafe fn impl_alloc_dst_box<T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Box<T, A>, AllocError> {
        if self.rejects_smart_ptr_align(layout.align()) {
            return Err(AllocError::ALIGNMENT_TOO_LARGE);
        }
        let meta_bytes = T::ALLOCATION_METADATA_BYTES;
        // Payload starts at the lowest layout-aligned offset >=
        // meta_bytes. For sized T (meta_bytes = 0) payload starts at 0.
        let payload_offset = if meta_bytes == 0 { 0 } else { meta_bytes.max(layout.align()) };
        // Keep the payload pointer inside the reservation for ZSTs.
        let value_bytes = layout.size().max(1);
        let total = payload_offset.checked_add(value_bytes).ok_or(AllocError::CAPACITY_OVERFLOW)?;
        // Include alignment slack so the retry fits the chosen chunk.
        let refill_hint = total.saturating_add(layout.align());
        let mut init = Some(init);
        loop {
            if let Some((reservation, chunk_ptr)) = self.current().try_alloc_with_chunk(total, layout.align().max(1)) {
                let init = init.take().expect("init taken twice");
                let chunk_ref = self.acquire_current_chunk_ref(chunk_ptr);
                // SAFETY: `reservation` is fresh exclusive storage; metadata
                // is written before `init` receives the fat payload pointer.
                let payload_nn =
                    unsafe { write_dst_prefix_and_init::<T>(reservation.as_non_null(), payload_offset, meta_bytes, metadata, init) };
                let _ = chunk_ref.forget();
                // SAFETY: `payload_nn` references initialized `T`; the
                // hosting chunk holds the new `Box`'s +1.
                return Ok(unsafe { Box::from_raw_with_metadata(payload_nn, metadata) });
            }
            if self.is_oversized(refill_hint) {
                let init = init.take().expect("init taken twice");
                return self.alloc_oversized_shared_with(refill_hint, |mutator, chunk_ptr| {
                    let (reservation, _chunk) = mutator
                        .try_alloc_with_chunk(total, layout.align().max(1))
                        .expect("dedicated oversized chunk sized to fit DST value + alignment slack");
                    let chunk_ref = acquire_chunk_ref::<A>(chunk_ptr);
                    // SAFETY: `reservation` is fresh exclusive storage from the
                    // dedicated oversized chunk; the DST metadata is written
                    // before `init` receives the fat payload pointer.
                    let payload_nn =
                        unsafe { write_dst_prefix_and_init::<T>(reservation.as_non_null(), payload_offset, meta_bytes, metadata, init) };
                    let _ = chunk_ref.forget();
                    // SAFETY: `payload_nn` references the now-initialized `T`;
                    // the oversized chunk holds the new `Box`'s `+1` (forgotten
                    // above), so `Box::from_raw_with_metadata` adopts sole
                    // ownership.
                    unsafe { Box::from_raw_with_metadata(payload_nn, metadata) }
                });
            }
            self.refill(refill_hint)?;
        }
    }

    /// Reserve a strong-prefixed `Arc`/`Rc` `T` slot in the current chunk,
    /// including any allocation-resident metadata, run `init` on a typed fat
    /// pointer, and adopt the result into `S`'s smart pointer.
    /// The smart pointer's `Drop` runs `drop_in_place::<T>` (which natively
    /// handles `?Sized`) on the last reference.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    #[inline]
    unsafe fn impl_alloc_dst_smart<S: Strong, T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<S::Ptr<T, A>, AllocError> {
        // SAFETY: forwarded to the raw helper, which shares this method's
        // contract on `layout` / `metadata` / `init`.
        let thin = unsafe { self.alloc_dst_smart_raw::<S, T>(layout, metadata, init) }?;
        // SAFETY: `alloc_dst_smart_raw` returns a thin pointer to a
        // fully-initialized `T` with a strong count of 1, and whose hosting
        // chunk it took a `+1` on. The supplied metadata describes that value.
        Ok(unsafe { S::adopt_with_metadata::<T, A>(thin, metadata) })
    }

    /// Raw DST smart allocation returning the thin payload pointer (before
    /// adoption). Split out so the single `S::adopt` lives in
    /// [`Self::impl_alloc_dst_smart`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    #[inline]
    unsafe fn alloc_dst_smart_raw<S: Strong, T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<NonNull<u8>, AllocError> {
        if self.rejects_smart_ptr_align(layout.align()) {
            return Err(AllocError::ALIGNMENT_TOO_LARGE);
        }
        let meta_bytes = T::ALLOCATION_METADATA_BYTES;
        let value_align = layout.align().max(1);
        // Keep the payload pointer inside the reservation for ZSTs.
        let payload_bytes = layout.size().max(1);
        let refill_hint = worst_case_strong_dst::<S>(payload_bytes, value_align, meta_bytes);

        let mut init = Some(init);
        loop {
            if let Some((value_ptr, chunk_ptr)) = self.current().try_alloc_arc_dst::<S>(payload_bytes, value_align, meta_bytes) {
                let init = init.take().expect("init taken twice");
                let chunk_ref = self.acquire_current_chunk_ref(chunk_ptr);
                // SAFETY: `value_ptr` is fresh payload storage with a
                // strong prefix; metadata is written before `init`.
                let payload_nn = unsafe { write_dst_meta_and_init::<T>(value_ptr, meta_bytes, metadata, init) };
                let _ = chunk_ref.forget();
                return Ok(payload_nn);
            }

            if self.is_oversized(refill_hint) {
                let init = init.take().expect("init taken twice");
                return self.alloc_oversized_shared_with(refill_hint, |mutator, chunk_ptr| {
                    let (value_ptr, _chunk) = mutator
                        .try_alloc_arc_dst::<S>(payload_bytes, value_align, meta_bytes)
                        .expect("dedicated oversized chunk sized to fit DST value + strong prefix");
                    let chunk_ref = acquire_chunk_ref::<A>(chunk_ptr);
                    // SAFETY: `value_ptr` is fresh payload storage with a strong
                    // prefix from the dedicated oversized chunk; the DST metadata
                    // is written before `init` populates the value.
                    let payload_nn = unsafe { write_dst_meta_and_init::<T>(value_ptr, meta_bytes, metadata, init) };
                    let _ = chunk_ref.forget();
                    payload_nn
                });
            }
            self.refill(refill_hint)?;
        }
    }
}

#[cfg(feature = "dst")]
impl<A: Allocator + Clone> Arena<A> {
    /// `Pin` variant of [`Self::alloc_dst_arc`]. Returns a pinned
    /// `Arc<T, A>` where the value's address is fixed in the arena
    /// and never moves until the last `Arc` clone is dropped.
    ///
    /// Typical use: pinning an `Arc<[T]>` whose slice contents must
    /// stay at a fixed address (e.g. for `Pin`-projecting code). Trait objects
    /// are also supported; their vtable is carried in the returned handle.
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let value = unsafe {
    ///     arena.alloc_dst_arc_pin::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    #[must_use]
    pub unsafe fn alloc_dst_arc_pin<T: ?Sized + Send + Sync + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Pin<Arc<T, A>>
    where
        A: Send + Sync + 'static,
    {
        // SAFETY: forwarded.
        // SAFETY: the new owner has not been exposed or cloned.
        unsafe { Arc::pin_fresh(self.alloc_dst_arc::<T>(layout, metadata, init)) }
    }

    /// Fallible variant of [`Self::alloc_dst_arc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::try_alloc_dst_arc`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let Ok(value) = (unsafe {
    ///     arena.try_alloc_dst_arc_pin::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// }) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_arc_pin<T: ?Sized + Send + Sync + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Pin<Arc<T, A>>, AllocError>
    where
        A: Send + Sync + 'static,
    {
        // SAFETY: forwarded.
        unsafe { self.try_alloc_dst_arc::<T>(layout, metadata, init) }.map(|owner| {
            // SAFETY: the new owner has not been exposed or cloned.
            unsafe { Arc::pin_fresh(owner) }
        })
    }

    /// `Pin` variant of [`Self::alloc_dst_rc`] (non-atomic).
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let value = unsafe {
    ///     arena.alloc_dst_rc_pin::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    #[must_use]
    pub unsafe fn alloc_dst_rc_pin<T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Pin<Rc<T, A>>
    where
        A: 'static,
    {
        // SAFETY: forwarded.
        // SAFETY: the new owner has not been exposed or cloned.
        unsafe { Rc::pin_fresh(self.alloc_dst_rc::<T>(layout, metadata, init)) }
    }

    /// Fallible variant of [`Self::alloc_dst_rc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_rc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let Ok(value) = (unsafe {
    ///     arena.try_alloc_dst_rc_pin::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// }) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_rc_pin<T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Pin<Rc<T, A>>, AllocError>
    where
        A: 'static,
    {
        // SAFETY: forwarded.
        unsafe { self.try_alloc_dst_rc::<T>(layout, metadata, init) }.map(|owner| {
            // SAFETY: the new owner has not been exposed or cloned.
            unsafe { Rc::pin_fresh(owner) }
        })
    }

    /// `Pin` variant of [`Self::alloc_dst_box`].
    ///
    /// Trait objects are supported; their vtable is carried in the returned
    /// handle.
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_box`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_box`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let value = unsafe {
    ///     arena.alloc_dst_box_pin::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    #[must_use]
    pub unsafe fn alloc_dst_box_pin<T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Pin<Box<T, A>>
    where
        A: 'static,
    {
        // SAFETY: forwarded.
        Box::into_pin(unsafe { self.alloc_dst_box::<T>(layout, metadata, init) })
    }

    /// Fallible variant of [`Self::alloc_dst_box_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_box`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::try_alloc_dst_box`].
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "dst")]
    /// # {
    /// use core::alloc::Layout;
    /// let arena = multitude::Arena::new();
    /// let source = [1_u32, 2, 3];
    /// let Ok(layout) = Layout::array::<u32>(source.len()) else {
    ///     panic!("slice layout overflow");
    /// };
    /// // SAFETY: the layout, metadata, and initializer describe `source`.
    /// let Ok(value) = (unsafe {
    ///     arena.try_alloc_dst_box_pin::<[u32]>(layout, source.len(), |dst| {
    ///         core::ptr::copy_nonoverlapping(source.as_ptr(), dst.cast::<u32>(), source.len());
    ///     })
    /// }) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, &source);
    /// # }
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_box_pin<T: ?Sized + SmartPointerPointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Pin<Box<T, A>>, AllocError>
    where
        A: 'static,
    {
        // SAFETY: forwarded.
        unsafe { self.try_alloc_dst_box::<T>(layout, metadata, init) }.map(Box::into_pin)
    }
}

/// Worst-case byte budget for a single strong-prefixed DST allocation under
/// policy `S` ([`AtomicStrong`](thin_dst::AtomicStrong) for `Arc`,
/// [`LocalStrong`](thin_dst::LocalStrong) for `Rc`): shared strong count +
/// allocation metadata + payload + front alignment slack (`S::block_align`).
/// Using `S::block_align` keeps the hint tight for `Rc`'s sub-4-byte alignments
/// instead of over-budgeting at the `Arc` 4-byte strong-count floor. (`Box` DST
/// is not strong-prefixed — it uses the separate `impl_alloc_dst_box` path.)
#[cfg_attr(test, mutants::skip)] // underestimating refill hint ⇒ refill spin
#[inline]
fn worst_case_strong_dst<S: Strong>(payload_bytes: usize, value_align: usize, meta_bytes: usize) -> usize {
    strong_prefix_bytes_for(value_align, meta_bytes)
        .saturating_add(payload_bytes)
        .saturating_add(S::block_align(value_align))
}

/// Write metadata, call `init` on the reconstructed fat pointer, and
/// return the thin payload pointer. Used by strong-prefixed `Arc<T>` DSTs.
///
/// # Safety
///
/// - `value_ptr` must be the payload pointer of a strong-prefixed `Arc`
///   reservation whose prefix has room for `meta_bytes` immediately
///   before it.
/// - `init` must initialize a valid `T` through the fat pointer it
///   receives.
#[inline(always)]
unsafe fn write_dst_meta_and_init<T: ?Sized + SmartPointerPointee>(
    value_ptr: NonNull<u8>,
    meta_bytes: usize,
    metadata: T::Metadata,
    init: impl FnOnce(*mut T),
) -> NonNull<u8> {
    // SAFETY: per the function contract. The metadata word sits in
    // `[value_ptr - meta_bytes, value_ptr)`, inside the reservation
    // prefix; `write_unaligned` tolerates any alignment. For sized T
    // (meta_bytes == 0) the write is skipped.
    let fat = unsafe {
        if meta_bytes != 0 {
            let prefix_ptr = value_ptr.as_ptr().sub(meta_bytes).cast::<T::Metadata>();
            ptr::write_unaligned(prefix_ptr, metadata);
        }
        ptr_meta::from_raw_parts_mut::<T>(value_ptr.as_ptr().cast::<()>(), metadata)
    };
    // Caller's contract: `init` writes a valid `T` through `fat`. If it
    // panics, callers' `ChunkRef` guard releases the chunk's `+1`.
    init(fat);
    value_ptr
}

/// `Box` DST variant of [`write_dst_meta_and_init`]. `Box` has no
/// strong-count prefix, so the reservation starts at the metadata region.
///
/// # Safety
///
/// - `base` must reference `payload_offset + layout.size()` bytes of
///   exclusively-owned chunk storage aligned to `layout.align()`.
/// - `payload_offset` must equal `meta_bytes.max(layout.align())` for
///   DST or `0` for sized `T`.
/// - `init` must initialize a valid `T` through the fat pointer.
#[inline(always)]
unsafe fn write_dst_prefix_and_init<T: ?Sized + SmartPointerPointee>(
    base: NonNull<u8>,
    payload_offset: usize,
    meta_bytes: usize,
    metadata: T::Metadata,
    init: impl FnOnce(*mut T),
) -> NonNull<u8> {
    // SAFETY: per the function contract. `byte_add(payload_offset)`
    // stays within the reservation. The prefix at `payload - meta_bytes`
    // lies in `[base, base + payload_offset)`. For sized T (meta_bytes
    // == 0) the prefix write is a no-op.
    let (payload_nn, fat) = unsafe {
        let payload_nn = base.byte_add(payload_offset);
        if meta_bytes != 0 {
            let prefix_ptr = payload_nn.as_ptr().sub(meta_bytes).cast::<T::Metadata>();
            ptr::write_unaligned(prefix_ptr, metadata);
        }
        let fat = ptr_meta::from_raw_parts_mut::<T>(payload_nn.as_ptr().cast::<()>(), metadata);
        (payload_nn, fat)
    };
    // Caller's contract: `init` writes a valid `T` through `fat`. If it
    // panics, callers' `ChunkRef` guard releases the chunk's `+1`.
    init(fat);
    payload_nn
}
