// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! DST (unsized) value allocation API on [`Arena`].
//!
//! Implements `alloc_dst_arc`, `alloc_dst_box` and their `try_*`
//! variants under the `dst` Cargo feature. The pointer-metadata is
//! stored verbatim in the chunk prefix (immediately before the
//! payload), so supported DSTs are those whose metadata is either
//! zero-sized (sized `T`) or `usize`-sized (slice DSTs and trait
//! objects). `Arc` runs `T`'s destructor eagerly on the last clone via
//! `drop_in_place::<T>`; `Box` does so in its own `Drop`. Neither
//! family caps the metadata width for `T: Drop`: both drop via
//! `drop_in_place` on a full-width fat pointer, so a `Drop` trait
//! object or a slice longer than `u16::MAX` is accepted by both.

use core::alloc::Layout;
use core::mem;
use core::pin::Pin;
use core::ptr::{self, NonNull};

use allocator_api2::alloc::Allocator;
use ptr_meta::Pointee;

use super::alloc_value::acquire_chunk_ref;
use super::{Arena, ExpectAlloc};
use crate::AllocError;
use crate::arc::Arc;
use crate::r#box::Box;
use crate::internal::constants::max_smart_ptr_align;
use crate::internal::thin_dst::{AtomicStrong, LocalStrong, Strong, strong_prefix_bytes_for};
use crate::rc::Rc;

/// Maximum `layout.align()` accepted by smart-pointer allocations.
/// Mirrors the constant of the same name in [`alloc_value`](super::alloc_value):
/// values must lie strictly inside the first `CHUNK_ALIGN` bytes of
/// their chunk so the header-recovery mask works.
const MAX_SMART_PTR_ALIGN: usize = max_smart_ptr_align();

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
    /// Panics if the backing allocator fails or if `layout.align()` is
    /// at least 32 KiB.
    ///
    /// # Safety
    ///
    /// - `layout` must exactly describe the size and alignment of the
    ///   constructed DST value (e.g., for `[U]` of length `n`,
    ///   `Layout::array::<U>(n).unwrap()`). Passing a smaller layout
    ///   would cause `init` to write past the reservation.
    /// - `init` must initialize all bytes covered by `layout` to a valid `T`.
    /// - `metadata` must be valid for the value just written.
    /// - `T::Metadata` must be either zero-sized (sized `T`) or
    ///   `usize`-sized (slice DSTs `[U]` and trait objects `dyn Trait`,
    ///   whose metadata is a slice length or vtable pointer).
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
    pub unsafe fn alloc_dst_arc<T: ?Sized + Send + Sync + Pointee>(
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
    /// Allocator panics propagate.
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
    pub unsafe fn try_alloc_dst_arc<T: ?Sized + Send + Sync + Pointee>(
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
    pub unsafe fn alloc_dst_box<T: ?Sized + Pointee>(&self, layout: Layout, metadata: T::Metadata, init: impl FnOnce(*mut T)) -> Box<T, A> {
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
    /// Allocator panics propagate.
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
    pub unsafe fn try_alloc_dst_box<T: ?Sized + Pointee>(
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
    /// Panics if the backing allocator fails or if `layout.align()` is at least 32 KiB.
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
    pub unsafe fn alloc_dst_rc<T: ?Sized + Pointee>(&self, layout: Layout, metadata: T::Metadata, init: impl FnOnce(*mut T)) -> Rc<T, A> {
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
    pub unsafe fn try_alloc_dst_rc<T: ?Sized + Pointee>(
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
    unsafe fn impl_alloc_dst_arc<T: ?Sized + Send + Sync + Pointee>(
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
    unsafe fn impl_alloc_dst_rc<T: ?Sized + Pointee>(
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
    /// Like `impl_alloc_dst_arc` but without the per-`Arc` strong-count
    /// prefix: [`Box::drop`] runs `drop_in_place::<T>` on the value
    /// pointer (which natively handles `?Sized`).
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_box`].
    #[inline]
    unsafe fn impl_alloc_dst_box<T: ?Sized + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Box<T, A>, AllocError> {
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            return Err(AllocError::ALIGNMENT_TOO_LARGE);
        }
        let meta_bytes = mem::size_of::<T::Metadata>();
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
                return Ok(unsafe { Box::from_raw(payload_nn) });
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
                    // above), so `Box::from_raw` adopts sole ownership.
                    unsafe { Box::from_raw(payload_nn) }
                });
            }
            self.refill(refill_hint)?;
        }
    }

    /// Reserve a strong-prefixed `Arc`/`Rc` `T` slot in the current chunk
    /// (per-handle strong count + `T::Metadata` prefix + payload), run `init`
    /// on a typed fat pointer, and adopt the result into `S`'s smart pointer.
    /// The smart pointer's `Drop` runs `drop_in_place::<T>` (which natively
    /// handles `?Sized`) on the last reference.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    #[inline]
    unsafe fn impl_alloc_dst_smart<S: Strong, T: ?Sized + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<S::Ptr<T, A>, AllocError> {
        // SAFETY: forwarded to the raw helper, which shares this method's
        // contract on `layout` / `metadata` / `init`.
        let thin = unsafe { self.alloc_dst_smart_raw::<S, T>(layout, metadata, init) }?;
        // SAFETY: `alloc_dst_smart_raw` returns a thin pointer to a
        // fully-initialized `T` whose chunk prefix carries `T::Metadata` and a
        // strong count of 1, and whose hosting chunk it took a `+1` on; the
        // pointer lies in the chunk's first tile. That is `S::adopt`'s contract.
        Ok(unsafe { S::adopt::<T, A>(thin) })
    }

    /// Raw DST smart allocation returning the thin payload pointer (before
    /// adoption). Split out so the single `S::adopt` lives in
    /// [`Self::impl_alloc_dst_smart`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    #[inline]
    unsafe fn alloc_dst_smart_raw<S: Strong, T: ?Sized + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<NonNull<u8>, AllocError> {
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            return Err(AllocError::ALIGNMENT_TOO_LARGE);
        }
        let meta_bytes = mem::size_of::<T::Metadata>();
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
    /// stay at a fixed address (e.g. for `Pin`-projecting code).
    /// Trait objects whose metadata is a vtable pointer are **not**
    /// supported (see [`Self::try_alloc_dst_arc`]'s safety contract).
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
    pub unsafe fn alloc_dst_arc_pin<T: ?Sized + Send + Sync + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Pin<Arc<T, A>>
    where
        A: Send + Sync + 'static,
    {
        // SAFETY: forwarded.
        Arc::into_pin(unsafe { self.alloc_dst_arc::<T>(layout, metadata, init) })
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
    pub unsafe fn try_alloc_dst_arc_pin<T: ?Sized + Send + Sync + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Pin<Arc<T, A>>, AllocError>
    where
        A: Send + Sync + 'static,
    {
        // SAFETY: forwarded.
        unsafe { self.try_alloc_dst_arc::<T>(layout, metadata, init) }.map(Arc::into_pin)
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
    pub unsafe fn alloc_dst_rc_pin<T: ?Sized + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Pin<Rc<T, A>>
    where
        A: 'static,
    {
        // SAFETY: forwarded.
        Rc::into_pin(unsafe { self.alloc_dst_rc::<T>(layout, metadata, init) })
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
    pub unsafe fn try_alloc_dst_rc_pin<T: ?Sized + Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Pin<Rc<T, A>>, AllocError>
    where
        A: 'static,
    {
        // SAFETY: forwarded.
        unsafe { self.try_alloc_dst_rc::<T>(layout, metadata, init) }.map(Rc::into_pin)
    }

    /// `Pin` variant of [`Self::alloc_dst_box`]. Trait objects are
    /// **not** supported (see [`Self::try_alloc_dst_arc`]'s safety
    /// contract); use the slice or sized variants.
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
    pub unsafe fn alloc_dst_box_pin<T: ?Sized + Pointee>(
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
    pub unsafe fn try_alloc_dst_box_pin<T: ?Sized + Pointee>(
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
/// [`LocalStrong`](thin_dst::LocalStrong) for `Rc`): per-handle strong count +
/// `T::Metadata` prefix + payload + front alignment slack (`S::block_align`).
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
unsafe fn write_dst_meta_and_init<T: ?Sized + Pointee>(
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
unsafe fn write_dst_prefix_and_init<T: ?Sized + Pointee>(
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
