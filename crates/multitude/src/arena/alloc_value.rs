// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Scalar value allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.

use allocator_api2::alloc::{AllocError, Allocator};

use super::{AllocFlavor, Arena};
use crate::arc::Arc;
use crate::r#box::Box;
use crate::rc::Rc;

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate `value` and return a thread-local reference-counted
    /// smart pointer to it.
    ///
    /// If `T` needs drop, a tiny entry is added to the owning chunk's
    /// drop list so `T::drop` runs exactly once when the chunk is
    /// reclaimed.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_rc`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let v = arena.alloc_rc(vec![1, 2, 3]);
    /// assert_eq!(v.len(), 3);
    /// ```
    #[inline]
    pub fn alloc_rc<T>(&self, value: T) -> Rc<T, A> {
        let ptr = self.alloc_inner_value_or_panic::<T>(value, AllocFlavor::Rc);
        // SAFETY: helper bumped the chunk's per-flavor +1 for this Rc.
        Rc::from_owned_in_chunk(unsafe { crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr) })
    }

    /// Fallible variant of [`Self::alloc_rc`].
    ///
    /// Returns Err([`AllocError`]) instead of panicking if the backing
    /// allocator fails. The supplied `value` is dropped on failure.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_rc<T>(&self, value: T) -> Result<Rc<T, A>, AllocError> {
        let ptr = self.try_alloc_inner_value::<T>(value, AllocFlavor::Rc)?;
        // SAFETY: helper bumped the chunk's per-flavor +1 for this Rc.
        Ok(Rc::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }

    /// Allocate the result of `f`, constructing it in place in the arena.
    ///
    /// Avoids a stack copy of `T`. The closure may freely allocate from
    /// this arena.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_rc_with`] for a fallible variant.
    ///
    /// ## Closure-panic safety
    ///
    /// If `f` panics, an internal panic guard releases the protective
    /// `+1` refcount taken before `f` ran. No drop entry is linked, so
    /// `T::drop` does not run on the partially-constructed value. The
    /// reserved bump bytes leak in-chunk until the chunk is reset or
    /// reclaimed; the chunk's refcount is *not* leaked.
    #[inline]
    pub fn alloc_rc_with<T, F: FnOnce() -> T>(&self, f: F) -> Rc<T, A> {
        let ptr = self.alloc_inner_with_or_panic::<T, _>(f, AllocFlavor::Rc);
        // SAFETY: helper bumped the chunk's per-flavor +1 for this Rc.
        Rc::from_owned_in_chunk(unsafe { crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr) })
    }

    /// Fallible variant of [`Self::alloc_rc_with`].
    ///
    /// Returns Err([`AllocError`]) instead of panicking if the backing
    /// allocator fails. The closure is not called on failure.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// Propagates panics from `f`. See
    /// [`alloc_rc_with`](Self::alloc_rc_with) for closure-panic
    /// reservation/refcount semantics.
    #[inline]
    pub fn try_alloc_rc_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Rc<T, A>, AllocError> {
        let ptr = self.try_alloc_inner_with::<T, _>(f, AllocFlavor::Rc)?;
        // SAFETY: `try_alloc_inner_with` produces a fresh `T` written
        // into `ptr` and bumps the chunk refcount by one — exactly
        // the +1 that `Rc<T, A>` owns.
        Ok(Rc::<T, A>::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }

    /// Allocate `value` and return a `Send + Sync` reference-counted smart pointer.
    ///
    /// Costs an atomic RMW per clone/drop.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if  `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_arc`] for a fallible variant.
    #[inline]
    pub fn alloc_arc<T: Send + Sync>(&self, value: T) -> Arc<T, A>
    where
        A: Send + Sync,
    {
        let ptr = self.alloc_inner_arc_with_or_panic::<T, _>(move || value);
        // SAFETY: helper bumped the chunk's per-flavor +1 for this Arc.
        Arc::from_owned_in_chunk(unsafe { crate::internal::owned_in_chunk::OwnedInSharedChunk::from_raw_alloc(ptr) })
    }

    /// Fallible variant of [`Self::alloc_arc`].
    ///
    /// Returns Err([`AllocError`]) instead of panicking if the backing
    /// allocator fails. The supplied `value` is dropped on failure.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_arc<T: Send + Sync>(&self, value: T) -> Result<Arc<T, A>, AllocError>
    where
        A: Send + Sync,
    {
        self.try_alloc_arc_with(move || value)
    }

    /// Allocate the result of `f` in a `Shared`-flavor chunk and return an [`Arc`].
    ///
    /// The returned [`Arc`] is safe for cross-thread sharing. The closure
    /// constructs the value in place — no stack copy of `T`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_arc_with`] for a fallible variant.
    #[inline]
    pub fn alloc_arc_with<T: Send + Sync, F: FnOnce() -> T>(&self, f: F) -> Arc<T, A>
    where
        A: Send + Sync,
    {
        let ptr = self.alloc_inner_arc_with_or_panic::<T, _>(f);
        // SAFETY: helper bumped the chunk's per-flavor +1 for this Arc.
        Arc::from_owned_in_chunk(unsafe { crate::internal::owned_in_chunk::OwnedInSharedChunk::from_raw_alloc(ptr) })
    }

    /// Fallible variant of [`Self::alloc_arc_with`].
    ///
    /// Returns Err([`AllocError`]) instead of panicking if the backing
    /// allocator cannot satisfy the request. The closure is not called on failure.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// Propagates panics from `f`.
    #[inline]
    pub fn try_alloc_arc_with<T: Send + Sync, F: FnOnce() -> T>(&self, f: F) -> Result<Arc<T, A>, AllocError>
    where
        A: Send + Sync,
    {
        let ptr = self.try_alloc_inner_arc_with::<T, _>(f)?;
        // SAFETY: `try_alloc_inner_arc_with` produces a fresh `T`
        // written into `ptr` and bumps the arena's `arcs_issued`
        // counter — accounting for this `Arc`'s logical refcount
        // against the chunk's deferred inflation.
        Ok(Arc::<T, A>::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInSharedChunk::from_raw_alloc(ptr)
        }))
    }

    /// Allocate `value` and return an owned, mutable [`Box`] smart pointer.
    ///
    /// `Drop` runs `T::drop` immediately when the smart pointer is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_box`] for a fallible variant.
    #[inline]
    pub fn alloc_box<T>(&self, value: T) -> Box<T, A> {
        let ptr = self.alloc_inner_value_or_panic::<T>(value, AllocFlavor::Box);
        // SAFETY: helper bumped the chunk's per-flavor +1 for this Box.
        Box::from_owned_in_chunk(unsafe { crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr) })
    }

    /// Fallible variant of [`Self::alloc_box`].
    ///
    /// Returns Err([`AllocError`]) instead of panicking if the backing
    /// allocator cannot satisfy the request. The supplied `value` is
    /// dropped on failure.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[inline]
    pub fn try_alloc_box<T>(&self, value: T) -> Result<Box<T, A>, AllocError> {
        let ptr = self.try_alloc_inner_value::<T>(value, AllocFlavor::Box)?;
        // SAFETY: helper bumped the chunk's per-flavor +1 for this Box.
        Ok(Box::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }

    /// Allocate the result of `f` in the arena and return an owned, mutable [`Box`].
    ///
    /// `Drop` runs `T::drop` immediately when the smart pointer is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_box_with`] for a fallible variant.
    ///
    /// ## Closure-panic safety
    ///
    /// If `f` panics, an internal panic guard releases the protective
    /// `+1` refcount taken before `f` ran. No drop entry is linked, so
    /// `T::drop` does not run on the partially-constructed value. The
    /// reserved bump bytes leak in-chunk until the chunk is reset or
    /// reclaimed; the chunk's refcount is *not* leaked.
    #[inline]
    pub fn alloc_box_with<T, F: FnOnce() -> T>(&self, f: F) -> Box<T, A> {
        let ptr = self.alloc_inner_with_or_panic::<T, _>(f, AllocFlavor::Box);
        // SAFETY: helper bumped the chunk's per-flavor +1 for this Box.
        Box::from_owned_in_chunk(unsafe { crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr) })
    }

    /// Fallible variant of [`Self::alloc_box_with`].
    ///
    /// Returns Err([`AllocError`]) instead of panicking if the backing
    /// allocator fails. The closure is not called on allocator failure.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// Propagates panics from `f`. See
    /// [`alloc_box_with`](Self::alloc_box_with) for closure-panic
    /// reservation/refcount semantics.
    #[inline]
    pub fn try_alloc_box_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Box<T, A>, AllocError> {
        let ptr = self.try_alloc_inner_with(f, AllocFlavor::Box)?;
        // SAFETY: `try_alloc_inner_with` returns a valid, initialized pointer
        // with a +1 refcount owned by the caller (Box).
        Ok(Box::from_owned_in_chunk(unsafe {
            crate::internal::owned_in_chunk::OwnedInLocalChunk::from_raw_alloc(ptr)
        }))
    }

    /// Allocate `value` and return a pinned [`Box<T, A>`](crate::Box).
    /// Mirror of `std::boxed::Box::pin`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    #[must_use]
    #[inline]
    pub fn alloc_box_pin<T>(&self, value: T) -> core::pin::Pin<Box<T, A>>
    where
        A: 'static,
    {
        Box::into_pin(self.alloc_box(value))
    }

    /// Fallible variant of [`Self::alloc_box_pin`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB. The supplied `value` is
    /// dropped on failure.
    #[inline]
    pub fn try_alloc_box_pin<T>(&self, value: T) -> Result<core::pin::Pin<Box<T, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_box(value).map(Box::into_pin)
    }

    /// Allocate the result of `f` in place and return a pinned
    /// [`Box<T, A>`](crate::Box). The closure may construct `!Unpin`
    /// types (e.g. self-referential futures) directly into the arena
    /// without first creating them on the stack.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    #[must_use]
    #[inline]
    pub fn alloc_box_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> core::pin::Pin<Box<T, A>>
    where
        A: 'static,
    {
        Box::into_pin(self.alloc_box_with(f))
    }

    /// Fallible variant of [`Self::alloc_box_pin_with`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    #[inline]
    pub fn try_alloc_box_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<core::pin::Pin<Box<T, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_box_with(f).map(Box::into_pin)
    }

    /// Allocate `value` and return a pinned [`Rc<T, A>`](crate::Rc).
    /// Mirror of `std::rc::Rc::pin`. Pin is preserved across `Rc::clone`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    #[must_use]
    #[inline]
    pub fn alloc_rc_pin<T>(&self, value: T) -> core::pin::Pin<Rc<T, A>>
    where
        A: 'static,
    {
        Rc::into_pin(self.alloc_rc(value))
    }

    /// Fallible variant of [`Self::alloc_rc_pin`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB. The supplied `value` is
    /// dropped on failure.
    #[inline]
    pub fn try_alloc_rc_pin<T>(&self, value: T) -> Result<core::pin::Pin<Rc<T, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_rc(value).map(Rc::into_pin)
    }

    /// Allocate the result of `f` in place and return a pinned
    /// [`Rc<T, A>`](crate::Rc). Useful for constructing `!Unpin`
    /// values without ever moving them.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    #[must_use]
    #[inline]
    pub fn alloc_rc_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> core::pin::Pin<Rc<T, A>>
    where
        A: 'static,
    {
        Rc::into_pin(self.alloc_rc_with(f))
    }

    /// Fallible variant of [`Self::alloc_rc_pin_with`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    #[inline]
    pub fn try_alloc_rc_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<core::pin::Pin<Rc<T, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_rc_with(f).map(Rc::into_pin)
    }

    /// Allocate `value` and return a pinned [`Arc<T, A>`](crate::Arc).
    /// Mirror of `std::sync::Arc::pin`. Pin is preserved across
    /// `Arc::clone` and is sound across threads.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    #[must_use]
    #[inline]
    pub fn alloc_arc_pin<T: Send + Sync>(&self, value: T) -> core::pin::Pin<Arc<T, A>>
    where
        A: Send + Sync + 'static,
    {
        Arc::into_pin(self.alloc_arc(value))
    }

    /// Fallible variant of [`Self::alloc_arc_pin`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB. The supplied `value` is
    /// dropped on failure.
    #[inline]
    pub fn try_alloc_arc_pin<T: Send + Sync>(&self, value: T) -> Result<core::pin::Pin<Arc<T, A>>, AllocError>
    where
        A: Send + Sync + 'static,
    {
        self.try_alloc_arc(value).map(Arc::into_pin)
    }

    /// Allocate the result of `f` in place and return a pinned
    /// [`Arc<T, A>`](crate::Arc). The dominant use case is
    /// `Arena::alloc_arc_pin_with(|| async move { ... })` for type-
    /// erased futures shared across threads.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    #[must_use]
    #[inline]
    pub fn alloc_arc_pin_with<T: Send + Sync, F: FnOnce() -> T>(&self, f: F) -> core::pin::Pin<Arc<T, A>>
    where
        A: Send + Sync + 'static,
    {
        Arc::into_pin(self.alloc_arc_with(f))
    }

    /// Fallible variant of [`Self::alloc_arc_pin_with`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    #[inline]
    pub fn try_alloc_arc_pin_with<T: Send + Sync, F: FnOnce() -> T>(&self, f: F) -> Result<core::pin::Pin<Arc<T, A>>, AllocError>
    where
        A: Send + Sync + 'static,
    {
        self.try_alloc_arc_with(f).map(Arc::into_pin)
    }

    /// Bump-allocate `value` and return a mutable reference whose
    /// lifetime is tied to `&self`. The cheapest allocation multitude
    /// offers — no refcount, no per-pointer bookkeeping. The borrow
    /// checker bounds the returned reference to the arena's lifetime.
    ///
    /// If `T: Drop`, a drop entry is registered in the chunk's drop
    /// list; `T::drop` runs at arena drop. (For per-pointer
    /// drop-on-drop semantics, use [`Self::alloc_box`] instead.)
    ///
    /// The chunk that hosts the value is "pinned" — it lives until
    /// arena drop (other allocations into the same chunk follow normal
    /// per-chunk reclamation rules and may extend its life past the
    /// arena via [`Rc`] / [`Arc`] smart pointers).
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let x: &mut u32 = arena.alloc(42);
    /// let y: &mut u32 = arena.alloc(100);
    /// *x += 1;
    /// *y += 1;
    /// assert_eq!(*x, 43);
    /// assert_eq!(*y, 101);
    /// ```
    #[expect(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn alloc<T>(&self, value: T) -> &mut T {
        let ptr = self.alloc_inner_value_or_panic::<T>(value, AllocFlavor::SimpleRef);
        // SAFETY: the value is initialized and the chunk is pinned for `&self`'s lifetime.
        unsafe { &mut *ptr.as_ptr() }
    }

    /// Fallible variant of [`Self::alloc`].
    ///
    /// Returns [`AllocError`] instead of panicking if the backing
    /// allocator cannot satisfy the request. The supplied `value` is
    /// dropped on failure. See [`Self::alloc`] for full semantics.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator cannot satisfy
    /// the request.
    #[expect(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc<T>(&self, value: T) -> Result<&mut T, AllocError> {
        let ptr = self.try_alloc_inner_value::<T>(value, AllocFlavor::SimpleRef)?;
        // SAFETY: chunk pinned via `is_pinned`; the returned `&mut T`
        // borrows from `&self` and so cannot outlive the arena.
        Ok(unsafe { &mut *ptr.as_ptr() })
    }

    /// Bump-allocate the result of `f`, constructing it in place in the arena.
    ///
    /// Avoids a stack copy of `T`. Returns a mutable reference whose
    /// lifetime is tied to `&self`. See [`Self::alloc`] for full semantics.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_with`] for a fallible variant.
    ///
    /// If `f` panics, the reservation is leaked in-chunk (no drop is registered, no
    /// refcount bumped) but the chunk itself reclaims normally.
    #[expect(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn alloc_with<T, F: FnOnce() -> T>(&self, f: F) -> &mut T {
        let ptr = self.alloc_inner_with_or_panic::<T, _>(f, AllocFlavor::SimpleRef);
        // SAFETY: the chunk hosting `ptr` is now pinned for the
        // arena's lifetime — see invariants on `try_alloc_inner_with`.
        unsafe { &mut *ptr.as_ptr() }
    }

    /// Fallible variant of [`Self::alloc_with`].
    ///
    /// Returns [`AllocError`] instead of panicking if the backing allocator
    /// fails. The closure is not called on allocator failure.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    #[expect(
        clippy::mut_from_ref,
        reason = "Simple references: each call returns a fresh, disjoint &mut T; the borrow checker treats the returned reference as exclusive of its own region but harmlessly aliasing-with-shared with the &Arena borrow"
    )]
    #[inline]
    pub fn try_alloc_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<&mut T, AllocError> {
        let ptr = self.try_alloc_inner_with::<T, _>(f, AllocFlavor::SimpleRef)?;
        // SAFETY: the chunk hosting `ptr` is now pinned for the
        // arena's lifetime — see invariants on `try_alloc_inner_with`.
        // The returned `&mut T` borrows from `&self`, so it can't
        // outlive the arena and therefore cannot outlive the chunk.
        Ok(unsafe { &mut *ptr.as_ptr() })
    }
}

#[cfg(test)]
mod owned_drop_tests {
    //! Exercises the `Drop` impls of `OwnedInLocalChunk` /
    //! `OwnedInSharedChunk` directly. In normal flow they are always
    //! consumed via `into_in_chunk()` by the smart-pointer constructors,
    //! so their `dec_ref`-via-drop paths are otherwise unreachable.
    //!
    //! The tests use a tracking backing allocator to observe the
    //! `dec_ref`'s downstream effect: if the `Drop` impl is removed
    //! (mutant), the chunk's `+1` stays elevated forever and the chunk
    //! cannot be reclaimed even after the arena drops — `live_bytes`
    //! stays nonzero. Under the original code the chunk returns to
    //! its provider on the refcount-zero transition and is freed when
    //! the provider's cache walks at arena drop.

    use alloc::rc::Rc as StdRc;
    use core::alloc::Layout;
    use core::cell::Cell;
    use core::ptr::NonNull;

    use allocator_api2::alloc::{AllocError, Allocator, Global};

    use super::AllocFlavor;
    use crate::ArenaBuilder;
    use crate::internal::owned_in_chunk::{OwnedInLocalChunk, OwnedInSharedChunk};

    /// Counts live backing-allocation bytes so the tests can observe
    /// chunk leaks (which would happen if the `OwnedIn*Chunk::drop`
    /// body were elided).
    #[derive(Clone)]
    struct LeakTracker {
        live_bytes: StdRc<Cell<isize>>,
    }

    impl LeakTracker {
        fn new() -> Self {
            Self {
                live_bytes: StdRc::new(Cell::new(0)),
            }
        }
        fn live_bytes(&self) -> isize {
            self.live_bytes.get()
        }
    }

    // SAFETY: forwards to `Global`; the counter is interior-mutable bookkeeping.
    unsafe impl Allocator for LeakTracker {
        #[expect(clippy::cast_possible_wrap, reason = "test allocator: chunk sizes fit in isize")]
        fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
            let p = Global.allocate(layout)?;
            self.live_bytes.set(self.live_bytes.get() + layout.size() as isize);
            Ok(p)
        }
        #[expect(clippy::cast_possible_wrap, reason = "test allocator: chunk sizes fit in isize")]
        unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
            // SAFETY: forwarded — caller's contract.
            unsafe { Global.deallocate(ptr, layout) };
            self.live_bytes.set(self.live_bytes.get() - layout.size() as isize);
        }
    }

    #[test]
    fn dropping_owned_local_chunk_releases_refcount() {
        let tracker = LeakTracker::new();
        {
            let arena = ArenaBuilder::new_in(tracker.clone()).build();
            let ptr = arena.try_alloc_inner_value::<u32>(7, AllocFlavor::Rc).unwrap();
            // SAFETY: `ptr` is a freshly-allocated `u32` in a live local
            // chunk and carries the `+1` reserved by `try_alloc_inner_value`.
            let owned: OwnedInLocalChunk<u32, LeakTracker> = unsafe { OwnedInLocalChunk::from_raw_alloc(ptr) };
            // Drop runs the chunk's `dec_ref`; `u32` has no `Drop`, so we
            // don't need to drop the value separately.
            drop(owned);
            drop(arena);
        }
        assert_eq!(
            tracker.live_bytes(),
            0,
            "OwnedInLocalChunk::drop must release the chunk's `+1` so the chunk can be freed"
        );
    }

    #[test]
    fn dropping_owned_shared_chunk_releases_refcount() {
        let tracker = LeakTracker::new();
        {
            let arena = ArenaBuilder::new_in(tracker.clone()).build();
            let ptr = arena.try_alloc_inner_arc_with::<u32, _>(|| 11).unwrap();
            // SAFETY: `ptr` is a freshly-allocated `u32` in a live shared
            // chunk and carries the `+1` reserved by `try_alloc_inner_arc_with`.
            let owned: OwnedInSharedChunk<u32, LeakTracker> = unsafe { OwnedInSharedChunk::from_raw_alloc(ptr) };
            drop(owned);
            drop(arena);
        }
        assert_eq!(
            tracker.live_bytes(),
            0,
            "OwnedInSharedChunk::drop must release the chunk's `+1` so the chunk can be freed"
        );
    }
}
