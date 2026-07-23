// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Scalar value allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.

use core::mem;
use core::pin::Pin;
use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::{Arena, ExpectAlloc};
use crate::arc::Arc;
use crate::r#box::Box;
use crate::internal::chunk_ref::ChunkRef;
use crate::internal::constants::max_smart_ptr_align;
use crate::internal::thin_dst::{AtomicStrong, LocalStrong};
use crate::internal::uninit::Uninit;
use crate::internal::{Chunk, thin_dst};
use crate::rc::Rc;
use crate::{Alloc, AllocError};

/// Worst-case bytes consumed by a single value allocation of type `T` in
/// a chunk: value bytes + alignment padding.
#[cfg_attr(test, mutants::skip)] // under-sized hint ⇒ refill loop spin (OOM)
#[inline]
const fn worst_case_payload<T>() -> usize {
    mem::size_of::<T>().saturating_add(mem::align_of::<T>())
}

/// Worst-case bytes consumed by a single strong-prefixed value allocation
/// under policy `S` ([`AtomicStrong`](thin_dst::AtomicStrong) for `Arc`,
/// [`LocalStrong`](thin_dst::LocalStrong) for `Rc`): the per-handle
/// strong-count prefix + value bytes + front alignment slack (`S::block_align`).
/// Using `S::block_align` keeps the hint tight for `Rc`'s sub-4-byte alignments
/// instead of over-budgeting at the `Arc` 4-byte strong-count floor. (`Box` is
/// not strong-prefixed — it allocates through the separate non-prefixed path.)
#[cfg_attr(test, mutants::skip)] // under-sized hint ⇒ refill loop spin (OOM)
#[inline]
fn worst_case_strong_payload<S: thin_dst::Strong, T>() -> usize {
    let align = mem::align_of::<T>();
    let value_bytes = if mem::size_of::<T>() == 0 { 1 } else { mem::size_of::<T>() };
    thin_dst::strong_prefix_bytes_for(align, 0)
        .saturating_add(value_bytes)
        .saturating_add(S::block_align(align))
}

/// Maximum `align_of::<T>()` accepted by smart-pointer allocations.
///
/// Boxes recover their chunk header by subtracting the value pointer's
/// offset within its `CHUNK_ALIGN` tile; for that step to land on the
/// header rather than the value itself, the value must lie strictly
/// inside the first `CHUNK_ALIGN` bytes. Keeping the alignment well
/// below `CHUNK_ALIGN` leaves room for the chunk header plus the
/// value itself in the dedicated oversized case.
pub(in crate::arena) const MAX_SMART_PTR_ALIGN: usize = max_smart_ptr_align();

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate `value` and return a `Send + Sync` reference-counted smart pointer.
    ///
    /// Cloning and dropping are **O(1)**. For a cheaper single-thread
    /// alternative, see [`Self::alloc_rc`].
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if  `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_arc`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_arc(7);
    /// assert_eq!(*value, 7);
    /// ```
    #[inline]
    pub fn alloc_arc<T: Send + Sync>(&self, value: T) -> Arc<T, A>
    where
        A: Send + Sync,
    {
        self.try_alloc_arc_with::<T, _>(move || value).expect_alloc()
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_arc(8) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 8);
    /// ```
    #[inline]
    pub fn try_alloc_arc<T: Send + Sync>(&self, value: T) -> Result<Arc<T, A>, AllocError>
    where
        A: Send + Sync,
    {
        self.try_alloc_arc_with::<T, _>(move || value)
    }

    /// Allocate the result of `f` in a chunk and return an [`Arc`].
    ///
    /// The returned [`Arc`] is safe for cross-thread sharing. The closure
    /// constructs the value in place — no stack copy of `T`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_arc_with`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_arc_with(|| String::from("arc"));
    /// assert_eq!(&*value, "arc");
    /// ```
    #[inline]
    pub fn alloc_arc_with<T: Send + Sync, F: FnOnce() -> T>(&self, f: F) -> Arc<T, A>
    where
        A: Send + Sync,
    {
        self.try_alloc_arc_with::<T, F>(f).expect_alloc()
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_arc_with(|| 9) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 9);
    /// ```
    #[inline]
    pub fn try_alloc_arc_with<T: Send + Sync, F: FnOnce() -> T>(&self, f: F) -> Result<Arc<T, A>, AllocError>
    where
        A: Send + Sync,
    {
        self.impl_alloc_smart_prefixed_with::<AtomicStrong, T, F>(f)
    }

    /// Allocate `value` in a non-atomic, reference-counted [`Rc`].
    ///
    /// Like [`Self::alloc_arc`] but `Rc` is [`!Send`](Send)/[`!Sync`](Sync), so
    /// `T` needs no `Send`/`Sync` bound, clone/drop are cheaper (non-atomic),
    /// and `str`/`[u8]` pack slightly tighter.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_rc`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_rc(String::from("local"));
    /// assert_eq!(&*value, "local");
    /// ```
    #[inline]
    pub fn alloc_rc<T>(&self, value: T) -> Rc<T, A> {
        self.try_alloc_rc_with::<T, _>(move || value).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_rc`].
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
    /// let Ok(value) = arena.try_alloc_rc(10) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 10);
    /// ```
    #[inline]
    pub fn try_alloc_rc<T>(&self, value: T) -> Result<Rc<T, A>, AllocError> {
        self.try_alloc_rc_with::<T, _>(move || value)
    }

    /// Allocate the result of `f` in a chunk and return an [`Rc`].
    ///
    /// See [`Self::alloc_rc`]; the closure constructs the value in place.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_rc_with`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_rc_with(|| 11);
    /// assert_eq!(*value, 11);
    /// ```
    #[inline]
    pub fn alloc_rc_with<T, F: FnOnce() -> T>(&self, f: F) -> Rc<T, A> {
        self.try_alloc_rc_with::<T, F>(f).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_rc_with`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// Propagates panics from `f`.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_rc_with(|| 12) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 12);
    /// ```
    #[inline]
    pub fn try_alloc_rc_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Rc<T, A>, AllocError> {
        self.impl_alloc_smart_prefixed_with::<LocalStrong, T, F>(f)
    }

    /// Allocate `value` and return an owned, mutable [`Box`] smart pointer.
    ///
    /// `Drop` runs `T::drop` immediately when the smart pointer is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_box`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_box(13);
    /// assert_eq!(*value, 13);
    /// ```
    #[inline]
    pub fn alloc_box<T>(&self, value: T) -> Box<T, A> {
        (self.impl_alloc_box_with::<T, _>(move || value)).expect_alloc()
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_box(14) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 14);
    /// ```
    #[inline]
    pub fn try_alloc_box<T>(&self, value: T) -> Result<Box<T, A>, AllocError> {
        self.impl_alloc_box_with::<T, _>(move || value)
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
    /// `+1` refcount taken before `f` ran. No destructor is queued, so
    /// `T::drop` does not run on the partially-constructed value. The
    /// reserved bump bytes leak in-chunk until the chunk is reset or
    /// reclaimed; the chunk's refcount is *not* leaked.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_box_with(|| String::from("boxed"));
    /// assert_eq!(&*value, "boxed");
    /// ```
    #[inline]
    pub fn alloc_box_with<T, F: FnOnce() -> T>(&self, f: F) -> Box<T, A> {
        (self.impl_alloc_box_with::<T, F>(f)).expect_alloc()
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_box_with(|| 15) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 15);
    /// ```
    #[inline]
    pub fn try_alloc_box_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Box<T, A>, AllocError> {
        self.impl_alloc_box_with::<T, F>(f)
    }

    /// Allocate `value` and return a pinned [`Box<T, A>`](crate::Box).
    /// Mirror of `std::boxed::Box::pin`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_box_pin(16);
    /// assert_eq!(*value, 16);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_box_pin<T>(&self, value: T) -> Pin<Box<T, A>>
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_box_pin(17) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 17);
    /// ```
    #[inline]
    pub fn try_alloc_box_pin<T>(&self, value: T) -> Result<Pin<Box<T, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_box(value).map(Box::into_pin)
    }

    /// Allocate the result of `f` in a pinned [`Box<T, A>`](crate::Box).
    ///
    /// The closure may construct `!Unpin`
    /// types (e.g. self-referential futures) directly into the arena
    /// without first creating them on the stack.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_box_pin_with(|| 18);
    /// assert_eq!(*value, 18);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_box_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> Pin<Box<T, A>>
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_box_pin_with(|| 19) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 19);
    /// ```
    #[inline]
    pub fn try_alloc_box_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Pin<Box<T, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_box_with(f).map(Box::into_pin)
    }

    /// Allocate `value` and return a pinned [`Arc<T, A>`](crate::Arc).
    /// Mirror of `std::sync::Arc::pin`. Pin is preserved across
    /// `Arc::clone` and is sound across threads.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_arc_pin(20);
    /// assert_eq!(*value, 20);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_arc_pin<T: Send + Sync>(&self, value: T) -> Pin<Arc<T, A>>
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_arc_pin(21) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 21);
    /// ```
    #[inline]
    pub fn try_alloc_arc_pin<T: Send + Sync>(&self, value: T) -> Result<Pin<Arc<T, A>>, AllocError>
    where
        A: Send + Sync + 'static,
    {
        self.try_alloc_arc(value).map(Arc::into_pin)
    }

    /// Allocate the result of `f` in a pinned [`Arc<T, A>`](crate::Arc).
    ///
    /// The dominant use case is
    /// `Arena::alloc_arc_pin_with(|| async move { ... })` for type-
    /// erased futures shared across threads.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_arc_pin_with(|| 22);
    /// assert_eq!(*value, 22);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_arc_pin_with<T: Send + Sync, F: FnOnce() -> T>(&self, f: F) -> Pin<Arc<T, A>>
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_arc_pin_with(|| 23) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 23);
    /// ```
    #[inline]
    pub fn try_alloc_arc_pin_with<T: Send + Sync, F: FnOnce() -> T>(&self, f: F) -> Result<Pin<Arc<T, A>>, AllocError>
    where
        A: Send + Sync + 'static,
    {
        self.try_alloc_arc_with(f).map(Arc::into_pin)
    }

    /// Allocate `value` and return a pinned [`Rc<T, A>`](crate::Rc).
    /// Mirror of `std::rc::Rc::pin`.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_rc_pin(24);
    /// assert_eq!(*value, 24);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_rc_pin<T>(&self, value: T) -> Pin<Rc<T, A>>
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_rc_pin(25) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 25);
    /// ```
    #[inline]
    pub fn try_alloc_rc_pin<T>(&self, value: T) -> Result<Pin<Rc<T, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_rc(value).map(Rc::into_pin)
    }

    /// Allocate the result of `f` in a pinned [`Rc<T, A>`](crate::Rc).
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if
    /// `align_of::<T>()` is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_rc_pin_with(|| 26);
    /// assert_eq!(*value, 26);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_rc_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> Pin<Rc<T, A>>
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_rc_pin_with(|| 27) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 27);
    /// ```
    #[inline]
    pub fn try_alloc_rc_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Pin<Rc<T, A>>, AllocError>
    where
        A: 'static,
    {
        self.try_alloc_rc_with(f).map(Rc::into_pin)
    }

    /// Bump-allocate `value` in an arena-lifetime [`Alloc`] handle.
    ///
    /// This is the cheapest owning allocation multitude offers — no
    /// refcount, no per-pointer bookkeeping. The borrow checker bounds the
    /// returned handle to the arena's lifetime.
    ///
    /// The returned [`Alloc<T>`](Alloc) dereferences to `T` and runs `T`'s
    /// destructor **eagerly** when it is dropped — never deferred to arena
    /// reset or teardown. (For an escapable, refcounted owner that can outlive
    /// the arena, use [`Self::alloc_box`] instead.)
    ///
    /// The chunk that hosts the value is "pinned" — it lives until arena drop
    /// (other allocations into the same chunk follow normal per-chunk
    /// reclamation rules and may extend its life past the arena via [`Arc`]
    /// smart pointers). The value's memory is reclaimed in bulk at
    /// [`Self::reset`] or arena drop, regardless of when the [`Alloc`] handle
    /// is dropped.
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
    /// let mut x = arena.alloc(42);
    /// let mut y = arena.alloc(100);
    /// *x += 1;
    /// *y += 1;
    /// assert_eq!(*x, 43);
    /// assert_eq!(*y, 101);
    /// ```
    #[inline]
    pub fn alloc<T>(&self, value: T) -> Alloc<'_, T> {
        self.impl_alloc_value_with::<T, _>(move || value).expect_alloc()
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc(28) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 28);
    /// ```
    #[inline]
    pub fn try_alloc<T>(&self, value: T) -> Result<Alloc<'_, T>, AllocError> {
        self.impl_alloc_value_with::<T, _>(move || value)
    }

    /// Bump-allocate the result of `f`, constructing it in place in the arena.
    ///
    /// Avoids a stack copy of `T`. Returns an [`Alloc`] handle whose lifetime
    /// is tied to `&self`. See [`Self::alloc`] for full semantics.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails or if the `align_of::<T>()` is at least 32 KiB.
    /// Use [`Self::try_alloc_with`] for a fallible variant.
    ///
    /// If `f` panics, the reservation is leaked in-chunk (no refcount bumped)
    /// but the chunk itself reclaims normally.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_with(|| String::from("built in place"));
    /// assert_eq!(&*value, "built in place");
    /// ```
    #[inline]
    pub fn alloc_with<T, F: FnOnce() -> T>(&self, f: F) -> Alloc<'_, T> {
        self.impl_alloc_value_with::<T, F>(f).expect_alloc()
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
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_with(|| 29) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(*value, 29);
    /// ```
    #[inline]
    pub fn try_alloc_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Alloc<'_, T>, AllocError> {
        self.impl_alloc_value_with::<T, F>(f)
    }

    /// Shared body for the scalar entry points (`alloc`, `try_alloc`,
    /// `alloc_with`, `try_alloc_with`): bump-allocate one `T`, write it, and
    /// adopt the slot into an owning [`Alloc`] handle.
    #[inline(always)]
    fn impl_alloc_value_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Alloc<'_, T>, AllocError> {
        let slot = self.alloc_value_with_raw::<T, F>(f)?;
        // SAFETY: `alloc_value_with_raw` returns the unique `&mut T` for a
        // freshly-written arena slot that the arena hands out exactly once and
        // never drops itself, so `Alloc` may take ownership and run its
        // destructor exactly once on drop.
        Ok(unsafe { Alloc::from_mut(slot) })
    }

    /// Raw scalar allocation returning the bare arena `&mut T` (before adoption
    /// into [`Alloc`]). Split out so the single `Alloc::from_mut` lives in
    /// [`Self::impl_alloc_value_with`].
    #[inline(always)]
    fn alloc_value_with_raw<T, F: FnOnce() -> T>(&self, f: F) -> Result<&mut T, AllocError> {
        if const { mem::align_of::<T>() >= MAX_SMART_PTR_ALIGN } {
            return Err(AllocError::ALIGNMENT_TOO_LARGE);
        }
        // `f` is moved into exactly one of the in-chunk or fallback paths.
        if let Some(u) = self.try_reserve_local::<T>() {
            return Ok(u.init(f()));
        }
        self.alloc_value_refill_with::<T, F>(f)
    }

    /// Cold continuation of [`Self::impl_alloc_value_with`]: refill the current
    /// chunk (or fall back to a dedicated oversized chunk) and retry until the
    /// reservation succeeds or the backing allocator fails.
    #[cold]
    #[inline(never)]
    fn alloc_value_refill_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<&mut T, AllocError> {
        // `f` is only invoked on the success arms that `return`, so it
        // is never moved on the fall-through path.
        loop {
            if let Some(u) = self.try_reserve_local::<T>() {
                return Ok(u.init(f()));
            }
            let wcp = worst_case_payload::<T>();
            if self.is_oversized(wcp) {
                return self.alloc_oversized_value_with::<T, F>(wcp, f);
            }
            self.refill(wcp)?;
        }
    }

    /// Out-of-line oversized-value fallback for
    /// [`Self::impl_alloc_value_with`], retaining the chunk after direct
    /// initialization.
    #[cold]
    #[inline(never)]
    #[expect(
        clippy::mut_from_ref,
        reason = "internal helper hands out a fresh, disjoint arena slot per call; the returned &mut is wrapped in an owning Alloc at the public boundary"
    )]
    fn alloc_oversized_value_with<T, F: FnOnce() -> T>(&self, wcp: usize, f: F) -> Result<&mut T, AllocError> {
        let mutator = self.acquire_oversized_local_mutator(wcp)?;
        let ticket = mutator
            .try_alloc_uninit::<T>()
            .expect("dedicated oversized chunk sized to fit one value");
        let value_ptr = ticket.init_raw(f());
        self.retain_oversized_local_mutator(mutator);
        // SAFETY: the chunk is retained in `retired_local` for the
        // `&self` borrow, so `value_ptr` stays valid; the value is
        // freshly initialized and uniquely held.
        Ok(unsafe { &mut *value_ptr.as_ptr() })
    }

    /// Shared fast-path body for the `alloc_box` family.
    ///
    /// Delegates to [`Self::impl_alloc_smart_with`] and wraps the
    /// resulting value pointer in a [`Box`].
    #[inline(always)]
    fn impl_alloc_box_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Box<T, A>, AllocError> {
        // SAFETY: `impl_alloc_smart_with` returns a `NonNull<T>` to a
        // freshly-written `T` whose containing chunk has just been
        // bumped by +1 in the new `Box`'s name. `Box` runs `T::drop`
        // eagerly in its own `Drop` and adopts that +1 via
        // `Box::from_raw`.
        self.impl_alloc_smart_with::<T, F>(f)
            .map(|ptr| unsafe { Box::from_raw(ptr.cast::<u8>()) })
    }

    /// Shared fast-path body for the `alloc_arc` / `alloc_rc` families,
    /// parameterized by the [`Strong`](thin_dst::Strong) count policy.
    ///
    /// Unlike [`Box`], an [`Arc`]/[`Rc`](crate::Rc) reserves a per-handle strong
    /// reference count in the chunk prefix (initialized to `1`), takes one chunk
    /// refcount for the whole family, and runs `T::drop` eagerly when the strong
    /// count reaches zero.
    #[inline(always)]
    fn impl_alloc_smart_prefixed_with<S: thin_dst::Strong, T, F: FnOnce() -> T>(&self, f: F) -> Result<S::Ptr<T, A>, AllocError> {
        let thin = self.alloc_smart_prefixed_with_raw::<S, T, F>(f)?;
        // SAFETY: `alloc_smart_prefixed_with_raw` returns a thin pointer to a
        // freshly-written `T` whose chunk prefix holds a strong count of 1 and
        // whose hosting chunk it took a `+1` on; the pointer lies in the chunk's
        // first tile. That is exactly `S::adopt`'s contract.
        Ok(unsafe { S::adopt::<T, A>(thin) })
    }

    /// Raw scalar smart allocation returning the thin payload pointer (before
    /// adoption). Split out so the single `S::adopt` lives in
    /// [`Self::impl_alloc_smart_prefixed_with`].
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // routing-predicate mutations ⇒ refill spin (OOM)
    fn alloc_smart_prefixed_with_raw<S: thin_dst::Strong, T, F: FnOnce() -> T>(&self, f: F) -> Result<NonNull<u8>, AllocError> {
        if const { mem::align_of::<T>() >= MAX_SMART_PTR_ALIGN } {
            return Err(AllocError::ALIGNMENT_TOO_LARGE);
        }
        let mut f = Some(f);
        loop {
            if let Some((uninit, chunk_ptr)) = self.try_reserve_arc_value::<S, T>() {
                let chunk_ref = self.acquire_current_chunk_ref(chunk_ptr);
                let f = f.take().expect("closure taken twice");
                let ptr = init_smart_slot::<T, A, _>(uninit, chunk_ref, f);
                // The strong prefix was written (count = 1) and the chunk holds
                // a fresh +1 for this smart-pointer family.
                return Ok(ptr.cast::<u8>());
            }
            let wcp = worst_case_strong_payload::<S, T>();
            if self.is_oversized(wcp) {
                let f = f.take().expect("closure taken twice");
                return self.alloc_oversized_smart_prefixed_with::<S, T, F>(wcp, f);
            }
            self.refill(wcp)?;
        }
    }

    /// Bump-allocates `T` in the arena's current chunk for a
    /// [`Box`], takes a +1 refcount on that chunk, and writes the value
    /// into the reservation. [`Box`] runs `T::drop` eagerly in its own
    /// `Drop`.
    ///
    /// Rejects alignments at or above [`MAX_SMART_PTR_ALIGN`]: such
    /// values cannot live inside the first [`CHUNK_ALIGN`] bytes of a
    /// chunk, which would break the header-recovery mask used by the
    /// smart pointers' `Drop` impls.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // routing-predicate mutations ⇒ refill spin (OOM)
    fn impl_alloc_smart_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<NonNull<T>, AllocError> {
        if const { mem::align_of::<T>() >= MAX_SMART_PTR_ALIGN } {
            return Err(AllocError::ALIGNMENT_TOO_LARGE);
        }
        loop {
            // A non-drop ZST allocation does not advance the bump cursor
            // (`try_alloc(0, _)` is a no-op), so back-to-back handouts
            // would never refill the chunk. The per-allocation handout
            // count draws down the pre-credited ref surplus; an unbounded
            // run could exhaust it (use-after-free) or underflow the
            // surplus reconciliation at retire (double-free). Pre-reserve
            // a 1-byte tag so each such handout advances the cursor,
            // bounding per-chunk handouts to the chunk capacity.
            if const { mem::size_of::<T>() == 0 } && self.current().try_alloc(1, 1).is_none() {
                self.refill(worst_case_payload::<T>())?;
                continue;
            }
            if let Some((uninit, chunk_ptr)) = self.try_reserve_shared::<T>() {
                let chunk_ref = self.acquire_current_chunk_ref(chunk_ptr);
                return Ok(init_smart_slot::<T, A, F>(uninit, chunk_ref, f));
            }
            let wcp = worst_case_payload::<T>();
            if self.is_oversized(wcp) {
                return self.alloc_oversized_smart_with::<T, F>(wcp, f);
            }
            self.refill(wcp)?;
        }
    }

    /// Cold oversized-`Box` fallback for [`Self::impl_alloc_smart_with`].
    #[cold]
    #[inline(never)]
    fn alloc_oversized_smart_with<T, F: FnOnce() -> T>(&self, wcp: usize, f: F) -> Result<NonNull<T>, AllocError> {
        let (mutator, chunk_ptr) = self.acquire_oversized_shared_mutator(wcp)?;
        let ticket = mutator
            .try_alloc_uninit::<T>()
            .expect("dedicated oversized chunk sized to fit one value");
        let chunk_ref = acquire_chunk_ref::<A>(chunk_ptr);
        let ptr = init_smart_slot::<T, A, F>(ticket, chunk_ref, f);
        // `mutator` drops here, releasing its `+1`. The smart-pointer
        // `chunk_ref` taken above owns the surviving `+1`.
        drop(mutator);
        Ok(ptr)
    }

    /// Cold oversized fallback for [`Self::impl_alloc_smart_prefixed_with`].
    #[cold]
    #[inline(never)]
    fn alloc_oversized_smart_prefixed_with<S: thin_dst::Strong, T, F: FnOnce() -> T>(
        &self,
        wcp: usize,
        f: F,
    ) -> Result<NonNull<u8>, AllocError> {
        let (mutator, chunk_ptr) = self.acquire_oversized_shared_mutator(wcp)?;
        let (ticket, _chunk) = mutator
            .try_alloc_arc_value::<S, T>()
            .expect("dedicated oversized chunk sized to fit one smart value + strong prefix");
        let chunk_ref = acquire_chunk_ref::<A>(chunk_ptr);
        let ptr = init_smart_slot::<T, A, F>(ticket, chunk_ref, f);
        drop(mutator);
        // The strong prefix was written (count = 1) and the chunk holds a fresh
        // +1 for this smart-pointer family.
        Ok(ptr.cast::<u8>())
    }
}

/// writes the value produced by `f` into the reservation. Factored out
/// of [`Arena::impl_alloc_smart_with`] so the closure-panic path runs
/// the refcount-release guard.
#[inline(always)]
fn init_smart_slot<T, A: Allocator + Clone, F: FnOnce() -> T>(uninit: Uninit<'_, T>, chunk_ref: ChunkRef<A>, f: F) -> NonNull<T> {
    let value = f();
    let _ = chunk_ref.forget();
    uninit.init_raw(value)
}

/// Bumps the strong refcount on `chunk_ptr` and returns a
/// [`ChunkRef`](crate::internal::chunk_ref::ChunkRef) that owns the
/// fresh +1. Shared by [`Arena::init_box_slot`] and
/// [`Arena::init_arc_slot`] so the unsafe `inc_ref`/`adopt` pair lives
/// in one place.
#[inline(always)]
pub(crate) fn acquire_chunk_ref<A: Allocator + Clone>(chunk_ptr: NonNull<Chunk<A>>) -> ChunkRef<A> {
    // SAFETY: `chunk_ptr` belongs to a currently-installed shared
    // mutator and the arena holds a +1 on it for the duration of
    // `&self`; we bump for the soon-to-be smart pointer and adopt
    // that +1 into a `ChunkRef`. If the value-init closure panics,
    // the `ChunkRef` releases the bump during unwinding (the
    // reservation is leaked in-chunk per the documented panic
    // semantics of the `alloc_box_with` / `alloc_arc_with` family).
    unsafe {
        chunk_ptr.as_ref().inc_ref();
        ChunkRef::<A>::adopt(chunk_ptr)
    }
}
