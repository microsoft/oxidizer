// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Scalar value allocation helpers on [`Arena`].
//!
//! Public docs live on [`Arena`] itself.

use core::mem;
use core::pin::Pin;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::{Arena, ExpectAlloc};
use crate::arc::Arc;
use crate::r#box::Box;
use crate::internal::chunk_ref::ChunkRef;
use crate::internal::constants::max_smart_ptr_align;
use crate::internal::drop_entry::DropEntry;
use crate::internal::shared_chunk::SharedChunk;
use crate::internal::uninit::Uninit;
use crate::internal::{Chunk, thin_dst};

/// Worst-case bytes consumed by a single value allocation of type `T` in
/// a chunk: value bytes + alignment padding, plus one [`DropEntry`] slot
/// if `T` requires drop.
#[cfg_attr(test, mutants::skip)] // under-sized hint ⇒ refill loop spin (OOM)
#[inline]
const fn worst_case_payload<T>() -> usize {
    let base = mem::size_of::<T>().saturating_add(mem::align_of::<T>());
    if mem::needs_drop::<T>() {
        base.saturating_add(mem::size_of::<DropEntry>())
    } else {
        base
    }
}

/// Worst-case bytes consumed by a single `Arc<T>` value allocation: the
/// per-`Arc` strong-count prefix + value bytes + front alignment slack.
#[cfg_attr(test, mutants::skip)] // under-sized hint ⇒ refill loop spin (OOM)
#[inline]
const fn worst_case_arc_payload<T>() -> usize {
    let align = mem::align_of::<T>();
    let value_bytes = if mem::size_of::<T>() == 0 { 1 } else { mem::size_of::<T>() };
    thin_dst::strong_prefix_bytes_for(align, 0)
        .saturating_add(value_bytes)
        .saturating_add(thin_dst::arc_block_align(align))
}

/// Maximum `align_of::<T>()` accepted by smart-pointer allocations.
///
/// Boxes recover their chunk header by subtracting the value pointer's
/// offset within its `CHUNK_ALIGN` tile; for that step to land on the
/// header rather than the value itself, the value must lie strictly
/// inside the first `CHUNK_ALIGN` bytes. Keeping the alignment well
/// below `CHUNK_ALIGN` leaves room for the chunk header plus the
/// value itself in the dedicated oversized case.
pub(crate) const MAX_SMART_PTR_ALIGN: usize = max_smart_ptr_align();

impl<A: Allocator + Clone> Arena<A> {
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
        (self.impl_alloc_arc_with::<T, _>(move || value)).expect_alloc()
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
        self.impl_alloc_arc_with::<T, _>(move || value)
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
        (self.impl_alloc_arc_with::<T, F>(f)).expect_alloc()
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
        self.impl_alloc_arc_with::<T, F>(f)
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
    /// `+1` refcount taken before `f` ran. No drop entry is linked, so
    /// `T::drop` does not run on the partially-constructed value. The
    /// reserved bump bytes leak in-chunk until the chunk is reset or
    /// reclaimed; the chunk's refcount is *not* leaked.
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
    #[inline]
    pub fn try_alloc_box_pin<T>(&self, value: T) -> Result<Pin<Box<T, A>>, AllocError>
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
    #[inline]
    pub fn try_alloc_arc_pin<T: Send + Sync>(&self, value: T) -> Result<Pin<Arc<T, A>>, AllocError>
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
    #[inline]
    pub fn try_alloc_arc_pin_with<T: Send + Sync, F: FnOnce() -> T>(&self, f: F) -> Result<Pin<Arc<T, A>>, AllocError>
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
    /// arena via [`Arc`] smart pointers).
    ///
    /// # Why `T: Send`?
    ///
    /// At first glance the bound is surprising — single-threaded arena
    /// use feels like it shouldn't require it, and bump allocators such
    /// as `bumpalo` allocate without it. The difference is destructors.
    /// `bumpalo` leaks by default (it never runs `T::drop`), so a
    /// migrated value is only ever bytes that nobody touches. multitude
    /// instead registers a drop entry and runs `T::drop` **at arena
    /// drop**. Because [`Arena`] is itself [`Send`], that teardown — and
    /// therefore `T::drop` — may execute on a thread other than the one
    /// that constructed the value. For a thread-affine `!Send` type
    /// (e.g. holding an [`Rc`](std::rc::Rc) whose other clones live on
    /// the original thread) that would be unsound, so `alloc` requires
    /// `T: Send`.
    ///
    /// The bound is conservative: it is only strictly necessary for
    /// `T: Drop`, but a static bound can't be conditioned on
    /// `mem::needs_drop::<T>()` without specialization, so it is applied
    /// uniformly. If you need to arena-allocate a `!Send` value, hold it
    /// behind a smart pointer that runs its destructor eagerly on the
    /// owning thread (e.g. [`Self::alloc_box`]).
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
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn alloc<T: Send>(&self, value: T) -> &mut T {
        (self.impl_alloc_value_with::<T, _>(move || value)).expect_alloc()
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
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn try_alloc<T: Send>(&self, value: T) -> Result<&mut T, AllocError> {
        self.impl_alloc_value_with::<T, _>(move || value)
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
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline]
    pub fn alloc_with<T: Send, F: FnOnce() -> T>(&self, f: F) -> &mut T {
        // See `alloc` for why the `Err` arm uses `panic_alloc!()` rather than
        // `unsafe { unreachable_unchecked() }`.
        (self.impl_alloc_value_with::<T, F>(f)).expect_alloc()
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
    #[allow(
        clippy::mut_from_ref,
        reason = "Simple references: each call returns a fresh, disjoint &mut T; the borrow checker treats the returned reference as exclusive of its own region but harmlessly aliasing-with-shared with the &Arena borrow"
    )]
    #[inline]
    pub fn try_alloc_with<T: Send, F: FnOnce() -> T>(&self, f: F) -> Result<&mut T, AllocError> {
        self.impl_alloc_value_with::<T, F>(f)
    }

    /// Shared fast-path body for the scalar entry points (`alloc`, `try_alloc`,
    /// `alloc_with`, `try_alloc_with`). Specialized per-monomorphization
    /// via the const `needs_drop` branch.
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    #[inline(always)]
    fn impl_alloc_value_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<&mut T, AllocError> {
        if const { mem::align_of::<T>() >= MAX_SMART_PTR_ALIGN } {
            return Err(AllocError);
        }
        // `f` is only invoked on the success arms that `return`, so it
        // is never moved on the fall-through path.
        loop {
            if const { mem::needs_drop::<T>() } {
                if let Some(u) = self.try_reserve_local_with_drop::<T>() {
                    return Ok(u.init(f()));
                }
            } else if let Some(u) = self.try_reserve_local::<T>() {
                return Ok(u.init(f()));
            }
            let wcp = worst_case_payload::<T>();
            if self.is_oversized(wcp) {
                return self.alloc_oversized_value_with::<T, F>(wcp, f);
            }
            self.refill_local(wcp)?;
        }
    }

    /// Cold oversized-value fallback for [`Self::impl_alloc_value_with`].
    ///
    /// Kept `#[inline(never)]` so the fast-path body stays small
    /// enough for the public scalar entry points to inline into their
    /// callers; the bench shows that re-inlining this branch into
    /// `impl_alloc_value_with` blows `alloc`'s instruction budget past
    /// the inlining heuristic and turns every call site into a real
    /// function call.
    ///
    /// Closure-free in the user-`f` argument: capturing `f` inside an
    /// `impl FnOnce` passed to `alloc_oversized_local_with` would force
    /// `f`'s environment (e.g. `&loop_counter` for a default-by-ref
    /// capture) into an addressable stack slot, adding a per-iteration
    /// spill on the hot path even when this cold branch is never taken.
    #[cold]
    #[inline(never)]
    #[allow(clippy::mut_from_ref, reason = "Simple references: see Self::try_alloc_with")]
    fn alloc_oversized_value_with<T, F: FnOnce() -> T>(&self, wcp: usize, f: F) -> Result<&mut T, AllocError> {
        let mutator = self.acquire_oversized_local_mutator(wcp)?;
        let value_ptr = if const { mem::needs_drop::<T>() } {
            let ticket = mutator
                .try_alloc_uninit_with_drop::<T>()
                .expect("dedicated oversized chunk sized to fit one value + drop entry");
            ticket.init_raw(f())
        } else {
            let ticket = mutator
                .try_alloc_uninit::<T>()
                .expect("dedicated oversized chunk sized to fit one value");
            ticket.init_raw(f())
        };
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

    /// Shared fast-path body for the `alloc_arc` family.
    ///
    /// Unlike [`Box`], an [`Arc`] reserves a per-`Arc` strong reference
    /// count in the chunk prefix (initialized to `1`), takes one chunk
    /// refcount for the whole `Arc` family, and runs `T::drop` eagerly
    /// when the strong count reaches zero — never via a chunk
    /// drop-entry.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // routing-predicate mutations ⇒ refill spin (OOM)
    fn impl_alloc_arc_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Arc<T, A>, AllocError>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        if const { mem::align_of::<T>() >= MAX_SMART_PTR_ALIGN } {
            return Err(AllocError);
        }
        let mut f = Some(f);
        loop {
            if let Some((uninit, chunk_ptr)) = self.try_reserve_arc_value::<T>() {
                let chunk_ref = self.acquire_current_shared_chunk_ref(chunk_ptr);
                let f = f.take().expect("closure taken twice");
                let ptr = init_smart_slot::<T, A, _>(uninit, chunk_ref, f);
                // SAFETY: the strong prefix was written (count = 1) and the
                // chunk holds a fresh +1 for this `Arc` family.
                return Ok(unsafe { Arc::from_raw(ptr.cast::<u8>()) });
            }
            let wcp = worst_case_arc_payload::<T>();
            if self.is_oversized(wcp) {
                let f = f.take().expect("closure taken twice");
                return self.alloc_oversized_arc_with::<T, F>(wcp, f);
            }
            self.refill_shared(wcp)?;
        }
    }

    /// Bump-allocates `T` in the arena's current shared chunk for a
    /// [`Box`], takes a +1 refcount on that chunk, and writes the value
    /// into the reservation. [`Box`] runs `T::drop` eagerly in its own
    /// `Drop`, so no chunk drop entry is reserved.
    ///
    /// Rejects alignments at or above [`MAX_SMART_PTR_ALIGN`]: such
    /// values cannot live inside the first [`CHUNK_ALIGN`] bytes of a
    /// chunk, which would break the header-recovery mask used by the
    /// smart pointers' `Drop` impls.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // routing-predicate mutations ⇒ refill spin (OOM)
    fn impl_alloc_smart_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<NonNull<T>, AllocError> {
        if const { mem::align_of::<T>() >= MAX_SMART_PTR_ALIGN } {
            return Err(AllocError);
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
            if const { mem::size_of::<T>() == 0 } && self.current_shared().try_alloc(1, 1).is_none() {
                self.refill_shared(worst_case_payload::<T>())?;
                continue;
            }
            if let Some((uninit, chunk_ptr)) = self.try_reserve_shared::<T>() {
                let chunk_ref = self.acquire_current_shared_chunk_ref(chunk_ptr);
                return Ok(init_smart_slot::<T, A, F>(uninit, chunk_ref, f));
            }
            let wcp = worst_case_payload::<T>();
            if self.is_oversized(wcp) {
                return self.alloc_oversized_smart_with::<T, F>(wcp, f);
            }
            self.refill_shared(wcp)?;
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
        let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
        let ptr = init_smart_slot::<T, A, F>(ticket, chunk_ref, f);
        // `mutator` drops here, releasing its `+1`. The smart-pointer
        // `chunk_ref` taken above owns the surviving `+1`.
        drop(mutator);
        Ok(ptr)
    }

    /// Cold oversized-`Arc` fallback for [`Self::impl_alloc_arc_with`].
    #[cold]
    #[inline(never)]
    fn alloc_oversized_arc_with<T, F: FnOnce() -> T>(&self, wcp: usize, f: F) -> Result<Arc<T, A>, AllocError>
    where
        A: Send + Sync,
        T: Send + Sync,
    {
        let (mutator, chunk_ptr) = self.acquire_oversized_shared_mutator(wcp)?;
        let (ticket, _chunk) = mutator
            .try_alloc_arc_value::<T>()
            .expect("dedicated oversized chunk sized to fit one Arc value + strong prefix");
        let chunk_ref = acquire_shared_chunk_ref::<A>(chunk_ptr);
        let ptr = init_smart_slot::<T, A, F>(ticket, chunk_ref, f);
        drop(mutator);
        // SAFETY: the strong prefix was written (count = 1) and the chunk
        // holds a fresh +1 for this `Arc` family.
        Ok(unsafe { Arc::from_raw(ptr.cast::<u8>()) })
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
pub(crate) fn acquire_shared_chunk_ref<A: Allocator + Clone>(chunk_ptr: NonNull<SharedChunk<A>>) -> ChunkRef<A> {
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
