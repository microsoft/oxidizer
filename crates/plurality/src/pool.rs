// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "pointer-recovery and slot-lifecycle paths group tightly-coupled unsafe operations under a single documented safety invariant; one block per operation would duplicate that invariant and obscure it"
)]

use alloc::boxed::Box as AllocBox;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::mem::{MaybeUninit, needs_drop};
use core::ptr::{NonNull, drop_in_place};

use allocator_api2::alloc::{Allocator, Global};

use crate::alloced::Alloc;
use crate::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use crate::atomic::{AtomicU32, AtomicUsize, fence};
use crate::boxed::Box;
use crate::builder::PoolBuilder;
use crate::chunk::{ChunkHeader, header_of, slot_at};
use crate::error::AllocError;
use crate::rc::Rc;
use crate::slot::{FREE_END, MAX_POOL_SLOTS, SlotCell};
use crate::sync::Arc;

/// Shared, refcounted state behind a [`Pool`]. Outlives the `Pool` handle when
/// smart pointers are still alive.
#[repr(C)]
pub(crate) struct PoolCore {
    /// Head of the embedded global free list (`FREE_END` = empty / must grow).
    pub(crate) free_head: AtomicU32,
    /// `1` for the live `Pool` handle plus one per live refcounted allocation.
    pub(crate) pool_refcount: AtomicUsize,
    /// Returns the core to its concrete `PoolInner<T, A>` type for teardown.
    pub(crate) teardown: unsafe fn(NonNull<Self>),
}

/// RAII guard owning a freshly allocated, initialized chunk during the window
/// before it is published into the directory.
///
/// Publication pushes the chunk pointer into the directory `Vec`, which can
/// reallocate and panic on allocator failure. If it does, dropping this guard
/// deallocates the not-yet-reachable chunk instead of leaking it. On success
/// `grow` transfers ownership to the pool and `mem::forget`s the guard. Under
/// `loom`, drop also tears down the per-slot refcounts first.
struct ChunkAllocationGuard<'a, T, A: Allocator> {
    chunk: NonNull<ChunkHeader>,
    #[cfg(loom)]
    slots: u32,
    layout: Layout,
    allocator: &'a A,
    _marker: PhantomData<fn() -> T>,
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T, A: Allocator> Drop for ChunkAllocationGuard<'_, T, A> {
    #[cfg_attr(test, mutants::skip)] // Observable only if Vec::push panics after allocation; the mutant leaks memory.
    fn drop(&mut self) {
        // SAFETY: the guard exclusively owns a fully initialized but
        // unpublished chunk allocated with `layout`.
        unsafe {
            #[cfg(loom)]
            for i in 0..self.slots {
                let slot = slot_at::<T>(self.chunk, i as usize);
                drop_in_place(&raw mut (*slot.as_ptr()).refcount);
            }
            self.allocator.deallocate(self.chunk.cast::<u8>(), self.layout);
        }
    }
}

#[inline]
#[cfg_attr(test, mutants::skip)] // Differences occur only at the unallocatable u32 slot-index ceiling.
const fn unbounded_chunk_cap(chunk_size: u32) -> u64 {
    MAX_POOL_SLOTS / chunk_size as u64
}

/// Concrete pool state. `core` is first so its full-provenance pointer can be
/// cast back by the concrete teardown callback stored inside it.
#[repr(C)]
pub(crate) struct PoolInner<T, A> {
    pub(crate) core: PoolCore,
    /// Slots per chunk (a power of two).
    pub(crate) chunk_size: u32,
    /// `log2(chunk_size)`.
    pub(crate) shift: u32,
    /// `chunk_size - 1`.
    pub(crate) mask: u32,
    /// Optional cap on the number of chunks.
    pub(crate) max_chunks: Option<u32>,
    /// Number of chunks allocated so far.
    pub(crate) chunks_allocated: AtomicU32,
    /// Total bytes allocated from the underlying allocator over the pool's
    /// lifetime. Present, and accounted, only under the `stats` feature so a
    /// pool built without it carries no tracking state or overhead.
    #[cfg(feature = "stats")]
    pub(crate) bytes_allocated: AtomicUsize,
    /// Memory layout of one chunk (fixed, since `chunk_size` is fixed).
    pub(crate) chunk_layout: Layout,
    /// `chunk_index -> chunk base`. Written only on the allocator thread; read
    /// there on `pop` and (once quiescent) at teardown. `!Sync` is the gate.
    pub(crate) directory: UnsafeCell<Vec<NonNull<ChunkHeader>>>,
    /// Allocator used for chunk allocations.
    pub(crate) allocator: A,
    pub(crate) _marker: PhantomData<fn() -> T>,
}

/// A growable, fixed-slot object pool.
///
/// See the [crate-level documentation](crate) for the concurrency model. The
/// pool is `Send` (it can be moved between threads) but **not** `Sync` (only
/// one thread allocates at a time). It produces four handle types — `Box`,
/// `Alloc`, `Arc`, `Rc`. `Box` and `Arc` are `Send` (when `T` and the allocator
/// `A` are), so they may be dropped from any thread; `Alloc` and `Rc` are
/// `!Send` and stay on the allocating thread.
pub struct Pool<T, A: Allocator = Global> {
    inner: NonNull<PoolInner<T, A>>,
}

// SAFETY: all cross-thread state in `PoolInner` is atomic; the non-atomic
// directory is only ever touched by the single allocator thread (guaranteed by
// `!Sync`) or at teardown when the pool is quiescent.
unsafe impl<T: Send, A: Allocator + Send> Send for Pool<T, A> {}

impl<T, A: Allocator> core::fmt::Debug for Pool<T, A> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Pool")
            .field("chunk_size", &self.chunk_size())
            .field("max_chunks", &self.max_chunks())
            .field("chunks_allocated", &self.chunks_allocated())
            .field("len", &self.len())
            .finish()
    }
}

impl<T> Pool<T, Global> {
    /// Creates a pool with the default chunk size and unbounded growth.
    #[must_use]
    pub fn new() -> Self {
        PoolBuilder::new().build()
    }

    /// Starts a [`PoolBuilder`].
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Replacing the builder with Default is an unviable/equivalent mutant.
    pub fn builder() -> PoolBuilder<T, Global> {
        PoolBuilder::new()
    }
}

impl<T> Default for Pool<T, Global> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, A: Allocator> Pool<T, A> {
    pub(crate) fn from_inner(inner: NonNull<PoolInner<T, A>>) -> Self {
        Self { inner }
    }

    #[inline]
    fn inner(&self) -> &PoolInner<T, A> {
        // SAFETY: `inner` is valid while this `Pool` holds a pool refcount.
        unsafe { self.inner.as_ref() }
    }

    /// Slots per chunk.
    #[must_use]
    pub fn chunk_size(&self) -> u32 {
        self.inner().chunk_size
    }

    /// The chunk cap, if any.
    #[must_use]
    pub fn max_chunks(&self) -> Option<u32> {
        self.inner().max_chunks
    }

    /// Number of chunks allocated so far.
    #[must_use]
    pub fn chunks_allocated(&self) -> u32 {
        self.inner().chunks_allocated.load(Relaxed)
    }

    /// Snapshot of the pool's allocation statistics.
    ///
    /// See [`PoolStats`](crate::PoolStats) for the meaning of each field.
    /// Available under the `stats` crate feature.
    ///
    /// # Examples
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "stats")] {
    /// use plurality::Pool;
    ///
    /// let pool = Pool::<u64>::builder().chunk_size(4).build();
    /// assert_eq!(pool.stats().total_chunks_allocated, 0);
    ///
    /// let _held = pool.alloc_box(7);
    /// let stats = pool.stats();
    /// assert_eq!(stats.total_chunks_allocated, 1);
    /// assert!(stats.total_bytes_allocated > 0);
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "stats")]
    #[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
    #[must_use]
    pub fn stats(&self) -> crate::PoolStats {
        let inner = self.inner();
        crate::PoolStats {
            total_chunks_allocated: u64::from(inner.chunks_allocated.load(Relaxed)),
            total_bytes_allocated: inner.bytes_allocated.load(Relaxed) as u64,
        }
    }

    /// Total slots across allocated chunks (`chunks_allocated * chunk_size`).
    #[must_use]
    pub fn capacity(&self) -> u64 {
        u64::from(self.chunks_allocated()) * u64::from(self.chunk_size())
    }

    /// Maximum capacity (`max_chunks * chunk_size`), or `None` if unbounded.
    #[must_use]
    pub fn max_capacity(&self) -> Option<u64> {
        self.inner().max_chunks.map(|m| u64::from(m) * u64::from(self.chunk_size()))
    }

    /// Number of live refcounted allocations (`Box`/`Arc`/`Rc`). Approximate
    /// under concurrent frees.
    ///
    /// Lifetime-bound [`Alloc`] handles are **not** counted.
    #[must_use]
    pub fn len(&self) -> u64 {
        // pool_refcount = 1 (the Pool handle) + live refcounted allocations.
        self.inner().core.pool_refcount.load(Relaxed).saturating_sub(1) as u64
    }

    /// `true` if there are no live refcounted allocations (`Alloc` handles are
    /// not counted; see [`len`](Self::len)).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Free slots in already-allocated chunks. Approximate under concurrency;
    /// like [`len`](Self::len), it does not account for live `Alloc` handles.
    #[must_use]
    pub fn available(&self) -> u64 {
        self.capacity().saturating_sub(self.len())
    }

    // ─── Box<T> (unique owner) ───────────────────────────────────────────

    /// Allocates `value` and returns a unique [`Box`].
    ///
    /// # Panics
    /// Panics if the pool is full. Use [`try_alloc_box`](Self::try_alloc_box)
    /// to handle exhaustion.
    #[inline]
    pub fn alloc_box(&self, value: T) -> Box<T, A> {
        match self.try_alloc_box(value) {
            Ok(b) => b,
            Err(err) => pool_full(err),
        }
    }

    /// Allocates a value produced by `f` and returns a unique [`Box`]. `f` is
    /// not called if the pool is full.
    ///
    /// # Panics
    /// Panics if the pool is full.
    #[inline]
    pub fn alloc_box_with<F: FnOnce() -> T>(&self, f: F) -> Box<T, A> {
        match self.try_alloc_box_with(f) {
            Ok(b) => b,
            Err(err) => pool_full(err),
        }
    }

    /// Fallible [`alloc_box`](Self::alloc_box).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `value` is dropped.
    #[inline]
    pub fn try_alloc_box(&self, value: T) -> Result<Box<T, A>, AllocError> {
        match self.alloc_slot() {
            Ok(slot) => {
                // SAFETY: `slot` was just popped and is owned exclusively here.
                unsafe { self.occupy_box(slot, value) };
                Ok(Box::from_slot(slot))
            }
            Err(err) => Err(err),
        }
    }

    /// Fallible [`alloc_box_with`](Self::alloc_box_with).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `f` is not called.
    #[inline]
    pub fn try_alloc_box_with<F: FnOnce() -> T>(&self, f: F) -> Result<Box<T, A>, AllocError> {
        let mut uninit = self.try_alloc_uninit_box()?;
        // RAII `uninit` frees the slot if `f()` panics, so no capacity leak.
        uninit.write_value(f());
        // SAFETY: the value was just written.
        Ok(unsafe { uninit.assume_init() })
    }

    // ─── Arc<T> (shared, atomic) ─────────────────────────────────────────

    /// Allocates `value` and returns a shared [`Arc`].
    ///
    /// # Panics
    /// Panics if the pool is full.
    #[inline]
    pub fn alloc_arc(&self, value: T) -> Arc<T, A>
    where
        T: Send + Sync,
    {
        match self.try_alloc_arc(value) {
            Ok(a) => a,
            Err(err) => pool_full(err),
        }
    }

    /// Allocates a value produced by `f` and returns a shared [`Arc`].
    ///
    /// # Panics
    /// Panics if the pool is full.
    #[inline]
    pub fn alloc_arc_with<F: FnOnce() -> T>(&self, f: F) -> Arc<T, A>
    where
        T: Send + Sync,
    {
        match self.try_alloc_arc_with(f) {
            Ok(a) => a,
            Err(err) => pool_full(err),
        }
    }

    /// Fallible [`alloc_arc`](Self::alloc_arc).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `value` is dropped.
    #[inline]
    pub fn try_alloc_arc(&self, value: T) -> Result<Arc<T, A>, AllocError>
    where
        T: Send + Sync,
    {
        match self.alloc_slot() {
            Ok(slot) => {
                // SAFETY: `slot` was just popped and is owned exclusively here.
                unsafe { self.occupy(slot, value) };
                Ok(Arc::from_slot(slot))
            }
            Err(err) => Err(err),
        }
    }

    /// Fallible [`alloc_arc_with`](Self::alloc_arc_with).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `f` is not called.
    #[inline]
    pub fn try_alloc_arc_with<F: FnOnce() -> T>(&self, f: F) -> Result<Arc<T, A>, AllocError>
    where
        T: Send + Sync,
    {
        let mut uninit = self.try_alloc_uninit_arc()?;
        // RAII `uninit` frees the slot if `f()` panics, so no capacity leak.
        uninit.write_value(f());
        // SAFETY: the value was just written.
        Ok(unsafe { uninit.assume_init() })
    }

    // ─── Alloc<'pool, T> (unique, lifetime-bound, cheapest) ──────────────

    /// Allocates `value` and returns an [`Alloc`] — a unique handle that borrows
    /// the pool. It cannot outlive the pool, but is the cheapest handle.
    ///
    /// # Panics
    /// Panics if the pool is full.
    #[inline]
    pub fn alloc(&self, value: T) -> Alloc<'_, T, A> {
        match self.try_alloc(value) {
            Ok(a) => a,
            Err(err) => pool_full(err),
        }
    }

    /// Allocates a value produced by `f` and returns an [`Alloc`]. `f` is not
    /// called if the pool is full.
    ///
    /// # Panics
    /// Panics if the pool is full.
    #[inline]
    pub fn alloc_with<F: FnOnce() -> T>(&self, f: F) -> Alloc<'_, T, A> {
        match self.try_alloc_with(f) {
            Ok(a) => a,
            Err(err) => pool_full(err),
        }
    }

    /// Fallible [`alloc`](Self::alloc).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `value` is dropped.
    #[inline]
    pub fn try_alloc(&self, value: T) -> Result<Alloc<'_, T, A>, AllocError> {
        match self.alloc_slot() {
            Ok(slot) => {
                // SAFETY: `slot` was just popped and is owned exclusively here.
                unsafe { self.occupy_local(slot, value) };
                Ok(Alloc::from_slot(slot))
            }
            Err(err) => Err(err),
        }
    }

    /// Fallible [`alloc_with`](Self::alloc_with).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `f` is not called.
    #[inline]
    pub fn try_alloc_with<F: FnOnce() -> T>(&self, f: F) -> Result<Alloc<'_, T, A>, AllocError> {
        let mut uninit = self.try_alloc_uninit()?;
        // RAII `uninit` frees the slot if `f()` panics, so no capacity leak.
        uninit.write(f());
        // SAFETY: the value was just written.
        Ok(unsafe { uninit.assume_init() })
    }

    // ─── Rc<T> (shared, non-atomic refcount, !Send) ──────────────────────

    /// Allocates `value` and returns a shared, non-atomically refcounted [`Rc`].
    ///
    /// # Panics
    /// Panics if the pool is full.
    #[inline]
    pub fn alloc_rc(&self, value: T) -> Rc<T, A> {
        match self.try_alloc_rc(value) {
            Ok(r) => r,
            Err(err) => pool_full(err),
        }
    }

    /// Allocates a value produced by `f` and returns an [`Rc`].
    ///
    /// # Panics
    /// Panics if the pool is full.
    #[inline]
    pub fn alloc_rc_with<F: FnOnce() -> T>(&self, f: F) -> Rc<T, A> {
        match self.try_alloc_rc_with(f) {
            Ok(r) => r,
            Err(err) => pool_full(err),
        }
    }

    /// Fallible [`alloc_rc`](Self::alloc_rc).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `value` is dropped.
    #[inline]
    pub fn try_alloc_rc(&self, value: T) -> Result<Rc<T, A>, AllocError> {
        match self.alloc_slot() {
            Ok(slot) => {
                // SAFETY: `slot` was just popped and is owned exclusively here.
                unsafe { self.occupy(slot, value) };
                Ok(Rc::from_slot(slot))
            }
            Err(err) => Err(err),
        }
    }

    /// Fallible [`alloc_rc_with`](Self::alloc_rc_with).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `f` is not called.
    #[inline]
    pub fn try_alloc_rc_with<F: FnOnce() -> T>(&self, f: F) -> Result<Rc<T, A>, AllocError> {
        let mut uninit = self.try_alloc_uninit_rc()?;
        // RAII `uninit` frees the slot if `f()` panics, so no capacity leak.
        uninit.write_value(f());
        // SAFETY: the value was just written.
        Ok(unsafe { uninit.assume_init() })
    }

    // ─── uninitialized placement ─────────────────────────────────────────

    /// Reserves a slot and returns an uninitialized [`Box`], for placing a
    /// value directly into pool memory. Call
    /// [`assume_init`](crate::Box::assume_init) once written.
    ///
    /// # Panics
    /// Panics if the pool is full.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_box(&self) -> Box<MaybeUninit<T>, A> {
        match self.try_alloc_uninit_box() {
            Ok(b) => b,
            Err(err) => pool_full(err),
        }
    }

    /// Fallible [`alloc_uninit_box`](Self::alloc_uninit_box).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available.
    #[inline]
    pub fn try_alloc_uninit_box(&self) -> Result<Box<MaybeUninit<T>, A>, AllocError> {
        match self.alloc_slot() {
            Ok(slot) => {
                // A `Box` never reads the slot refcount, so (like `Alloc`) only
                // the pool refcount needs bumping here.
                self.bump_pool_ref();
                Ok(Box::from_slot(slot.cast::<SlotCell<MaybeUninit<T>>>()))
            }
            Err(err) => Err(err),
        }
    }

    /// Reserves a slot and returns an uninitialized [`Arc`]. Call
    /// [`assume_init`](crate::Arc::assume_init) once written.
    ///
    /// # Panics
    /// Panics if the pool is full.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_arc(&self) -> Arc<MaybeUninit<T>, A>
    where
        T: Send + Sync,
    {
        match self.try_alloc_uninit_arc() {
            Ok(a) => a,
            Err(err) => pool_full(err),
        }
    }

    /// Fallible [`alloc_uninit_arc`](Self::alloc_uninit_arc).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available.
    #[inline]
    pub fn try_alloc_uninit_arc(&self) -> Result<Arc<MaybeUninit<T>, A>, AllocError>
    where
        T: Send + Sync,
    {
        match self.alloc_slot() {
            Ok(slot) => {
                // SAFETY: freshly popped; mark occupied without writing a value.
                unsafe { self.mark_occupied(slot) };
                Ok(Arc::from_slot(slot.cast::<SlotCell<MaybeUninit<T>>>()))
            }
            Err(err) => Err(err),
        }
    }

    /// Reserves a slot and returns an uninitialized [`Alloc`].
    ///
    /// # Panics
    /// Panics if the pool is full.
    #[must_use]
    #[inline]
    pub fn alloc_uninit(&self) -> Alloc<'_, MaybeUninit<T>, A> {
        match self.try_alloc_uninit() {
            Ok(a) => a,
            Err(err) => pool_full(err),
        }
    }

    /// Fallible [`alloc_uninit`](Self::alloc_uninit).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available.
    #[inline]
    pub fn try_alloc_uninit(&self) -> Result<Alloc<'_, MaybeUninit<T>, A>, AllocError> {
        match self.alloc_slot() {
            Ok(slot) => {
                // An `Alloc` never reads the slot refcount (`push_free`
                // overwrites it on drop), so skip initializing it and
                // `pool_refcount`.
                Ok(Alloc::from_slot(slot.cast::<SlotCell<MaybeUninit<T>>>()))
            }
            Err(err) => Err(err),
        }
    }

    /// Reserves a slot and returns an uninitialized [`Rc`].
    ///
    /// # Panics
    /// Panics if the pool is full.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_rc(&self) -> Rc<MaybeUninit<T>, A> {
        match self.try_alloc_uninit_rc() {
            Ok(r) => r,
            Err(err) => pool_full(err),
        }
    }

    /// Fallible [`alloc_uninit_rc`](Self::alloc_uninit_rc).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available.
    #[inline]
    pub fn try_alloc_uninit_rc(&self) -> Result<Rc<MaybeUninit<T>, A>, AllocError> {
        match self.alloc_slot() {
            Ok(slot) => {
                // SAFETY: freshly popped; mark occupied without writing a value.
                unsafe { self.mark_occupied(slot) };
                Ok(Rc::from_slot(slot.cast::<SlotCell<MaybeUninit<T>>>()))
            }
            Err(err) => Err(err),
        }
    }

    // ─── internals ───────────────────────────────────────────────────────

    /// Writes `value` into a freshly popped slot, marks it occupied, and bumps
    /// the pool refcount.
    ///
    /// # Safety
    /// `slot` must have just been popped off the free list (no other reference
    /// to it exists).
    #[inline]
    unsafe fn occupy(&self, slot: NonNull<SlotCell<T>>, value: T) {
        // SAFETY: exclusive ownership of the freshly popped slot.
        unsafe {
            self.mark_occupied(slot);
            SlotCell::write_value(slot, value);
        }
    }

    /// Like `occupy`, but for a `Box`: bumps the pool refcount and writes the
    /// value **without** initializing the slot refcount. A `Box` is the unique
    /// owner and never reads that field (`push_free` overwrites it on drop), so,
    /// like `Alloc`, only the pool refcount needs maintaining.
    ///
    /// # Safety
    /// `slot` must have just been popped off the free list.
    #[inline]
    unsafe fn occupy_box(&self, slot: NonNull<SlotCell<T>>, value: T) {
        self.bump_pool_ref();
        // SAFETY: exclusive ownership of the freshly popped slot.
        unsafe { SlotCell::write_value(slot, value) };
    }

    /// Marks a freshly popped slot occupied (refcount = 1) and bumps the pool
    /// refcount, without writing a value. Used by the shared `Arc`/`Rc` paths.
    ///
    /// # Safety
    /// `slot` must have just been popped off the free list.
    #[inline]
    unsafe fn mark_occupied(&self, slot: NonNull<SlotCell<T>>) {
        // SAFETY: exclusive ownership of the freshly popped slot.
        unsafe { (*slot.as_ptr()).refcount.store(1, Relaxed) };
        self.bump_pool_ref();
    }

    /// Bumps the pool refcount for one new refcounted allocation
    /// (`Box`/`Arc`/`Rc`).
    #[inline]
    fn bump_pool_ref(&self) {
        let _ = self.inner().core.pool_refcount.fetch_add(1, Relaxed);
    }

    /// Like `occupy`, but for a lifetime-bound `Alloc`: writes the value
    /// **without** touching `pool_refcount` (the `Alloc`'s borrow already proves
    /// the pool outlives it) and **without** initializing the slot refcount. An
    /// `Alloc` is the unique owner and never reads that field; the free path
    /// ([`push_free`]) overwrites it on drop, so the stale free-list link left
    /// behind is a don't-care.
    ///
    /// # Safety
    /// `slot` must have just been popped off the free list.
    #[inline]
    #[expect(
        clippy::unused_self,
        reason = "kept as a method for symmetry with `occupy`; the `Alloc` path deliberately skips `pool_refcount` and the slot refcount"
    )]
    unsafe fn occupy_local(&self, slot: NonNull<SlotCell<T>>, value: T) {
        // SAFETY: exclusive ownership of the freshly popped slot.
        unsafe { SlotCell::write_value(slot, value) };
    }

    /// Pops a free slot, growing the pool if necessary. Returns `Err` only if
    /// the pool is full and cannot grow (see [`AllocError`] for the cause).
    #[inline]
    fn alloc_slot(&self) -> Result<NonNull<SlotCell<T>>, AllocError> {
        let inner = self.inner();
        loop {
            let head = inner.core.free_head.load(Acquire);
            if head == FREE_END {
                // `grow` reserves and returns the first slot of the new chunk
                // (or an `AllocError` if the pool can't grow).
                return self.grow();
            }
            // SAFETY: `head` is a valid global index currently on the free list.
            let slot = unsafe { self.slot_for_global(head) };
            // SAFETY: a free slot's refcount field holds the next-free link.
            let next = unsafe { (*slot.as_ptr()).refcount.load(Relaxed) };
            if inner.core.free_head.compare_exchange_weak(head, next, AcqRel, Acquire).is_ok() {
                return Ok(slot);
            }
        }
    }

    /// Maps a global slot index to its slot pointer via the directory. Only
    /// called on the (single) allocator thread.
    ///
    /// # Safety
    /// `g` must be a valid global index for an allocated chunk.
    #[inline]
    unsafe fn slot_for_global(&self, g: u32) -> NonNull<SlotCell<T>> {
        let inner = self.inner();
        let chunk_no = (g >> inner.shift) as usize;
        let offset = (g & inner.mask) as usize;
        // SAFETY: single-thread directory access; `chunk_no = g / chunk_size` is
        // `< chunks_allocated == directory.len()` for any valid free-list index.
        let chunk = unsafe {
            let dir = &*inner.directory.get();
            *dir.get_unchecked(chunk_no)
        };
        // SAFETY: `offset < chunk_size`.
        unsafe { slot_at::<T>(chunk, offset) }
    }

    /// Allocates and installs one new chunk, reserves its first slot for the
    /// caller, and splices the rest onto the free list. Returns the reserved
    /// slot, or an [`AllocError`] identifying why the pool cannot grow (capacity
    /// limit vs. allocator failure). Runs only on the allocator thread.
    #[cold]
    #[inline(never)]
    fn grow(&self) -> Result<NonNull<SlotCell<T>>, AllocError> {
        let inner = self.inner();
        let chunks = inner.chunks_allocated.load(Relaxed);
        let n = inner.chunk_size;
        // Cap = the user's `max_chunks`, or for an unbounded pool the chunk count
        // that keeps every global index below the `FREE_END` sentinel.
        let cap = inner.max_chunks.map_or_else(|| unbounded_chunk_cap(n), u64::from);
        if u64::from(chunks) >= cap {
            return Err(AllocError::CAPACITY_EXHAUSTED);
        }
        let base_index = chunks * n;

        let ptr = match inner.allocator.allocate(inner.chunk_layout) {
            Ok(p) => p.cast::<ChunkHeader>(),
            Err(_) => return Err(AllocError::ALLOCATOR_FAILED),
        };

        // SAFETY: `ptr` is a fresh, exclusively owned allocation sized for one
        // chunk; the header and all slots are initialized before publishing.
        // Each slot links to `i + 1`; the last link and slot 0 are fixed up by
        // the splice and caller below.
        unsafe {
            ptr.as_ptr().write(ChunkHeader {
                // `core` is the first `#[repr(C)]` field. Cast the full inner
                // pointer rather than borrowing the field so provenance still
                // covers the complete pool allocation for concrete teardown.
                pool: self.inner.cast::<PoolCore>(),
                base_index,
                chunk_index: chunks,
            });
            for i in 0..n {
                let slot = slot_at::<T>(ptr, i as usize);
                slot.as_ptr().write(SlotCell {
                    value: UnsafeCell::new(MaybeUninit::uninit()),
                    refcount: AtomicU32::new(base_index + i + 1),
                    index: i,
                });
            }
            let guard = ChunkAllocationGuard::<T, A> {
                chunk: ptr,
                #[cfg(loom)]
                slots: n,
                layout: inner.chunk_layout,
                allocator: &inner.allocator,
                _marker: PhantomData,
            };
            (&mut *inner.directory.get()).push(ptr);
            // Directory publication transferred ownership to the pool.
            core::mem::forget(guard);
        }
        inner.chunks_allocated.store(chunks + 1, Release);
        // `Relaxed` suffices: the counter is only read via `stats()`, never to
        // establish a happens-before relationship.
        #[cfg(feature = "stats")]
        inner.bytes_allocated.fetch_add(inner.chunk_layout.size(), Relaxed);

        // Splice the free slots (base_index+1 .. base_index+n-1) onto the head;
        // slot `base_index` is returned to the caller.
        if n > 1 {
            // SAFETY: `ptr` chunk is live; its last slot is index n-1.
            let last = unsafe { slot_at::<T>(ptr, (n - 1) as usize) };
            // SAFETY: `last` is the new chunk's (still-private) final slot.
            unsafe { splice_chain(&inner.core.free_head, last, base_index + 1) };
        }
        // SAFETY: slot 0 of the new chunk; never published, so exclusively ours.
        Ok(unsafe { slot_at::<T>(ptr, 0) })
    }
}

/// Splices a freshly built chunk's free chain onto the global free list by
/// pointing its last slot at the current head and CAS-ing the head to the
/// chain's first index.
///
/// The CAS-retry branch is excluded from coverage: it only fires when a
/// concurrent free races this otherwise single-threaded splice.
///
/// # Safety
/// `last` must be the final slot of a fully-initialized, not-yet-published
/// chunk whose first global index is `base_index`.
#[cfg_attr(coverage_nightly, coverage(off))]
unsafe fn splice_chain<T>(free_head: &AtomicU32, last: NonNull<SlotCell<T>>, base_index: u32) {
    loop {
        let head = free_head.load(Acquire);
        // SAFETY: the new chain is private until the CAS publishes it.
        unsafe { (*last.as_ptr()).refcount.store(head, Relaxed) };
        if free_head.compare_exchange_weak(head, base_index, AcqRel, Acquire).is_ok() {
            break;
        }
    }
}

impl<T, A: Allocator> Drop for Pool<T, A> {
    fn drop(&mut self) {
        let inner = self.inner();
        if inner.core.pool_refcount.fetch_sub(1, Release) == 1 {
            fence(Acquire);
            // SAFETY: refcount hit zero, so we own the inner exclusively.
            unsafe { teardown(self.inner) };
        }
    }
}

/// Pushes a freed slot back onto the MPSC free list and returns the owning
/// pool. The load only needs a recent head value (a stale one just makes the CAS
/// retry); the CAS `Release` publishes the link store to the consumer's
/// `Acquire` load on pop.
///
/// # Safety
/// `slot` must be an occupied slot whose value has already been dropped.
///
/// Coverage is disabled because this path serves only `Alloc`, which is
/// `!Send`; its weak-CAS retry cannot be forced without a spurious failure.
#[inline]
#[cfg_attr(coverage_nightly, coverage(off))]
unsafe fn push_free<T>(slot: NonNull<SlotCell<T>>) -> NonNull<PoolCore> {
    // SAFETY: recovery is valid for any live slot from this crate.
    unsafe {
        let index = (*slot.as_ptr()).index;
        let header = header_of::<T>(slot, index);
        let pool = (*header.as_ptr()).pool;
        let global = (*header.as_ptr()).base_index + index;
        let inner = pool.as_ref();

        loop {
            let head = inner.free_head.load(Relaxed);
            (*slot.as_ptr()).refcount.store(head, Relaxed);
            if inner.free_head.compare_exchange_weak(head, global, Release, Relaxed).is_ok() {
                break;
            }
        }
        pool
    }
}

/// Like [`free_slot_local`] but for a lifetime-bound `Alloc`: pushes the slot
/// back **without** touching `pool_refcount` (the `Alloc` never held one). The
/// pool's lifetime guarantees the inner is still alive, so no teardown check.
///
/// # Safety
/// `slot` must be an occupied slot whose value has already been dropped.
#[inline]
unsafe fn free_slot_local<T>(slot: NonNull<SlotCell<T>>) {
    // SAFETY: recovery is valid for any live slot from this crate.
    unsafe {
        let _ = push_free::<T>(slot);
    }
}

/// Rounds `x` up to a multiple of `align` (a power of two).
#[inline]
const fn round_up(x: usize, align: usize) -> usize {
    (x + (align - 1)) & !(align - 1)
}

struct ErasedSlotGuard {
    value: NonNull<u8>,
    size: usize,
    align: usize,
}

impl Drop for ErasedSlotGuard {
    fn drop(&mut self) {
        // SAFETY: this guard is created only for the final owner of an occupied
        // slot and runs after normal or unwinding destruction of its value.
        unsafe { free_slot_erased(self.value, self.size, self.align) };
    }
}

struct LocalSlotGuard<T> {
    slot: NonNull<SlotCell<T>>,
}

impl<T> Drop for LocalSlotGuard<T> {
    fn drop(&mut self) {
        // SAFETY: this guard is created only for the unique local owner and runs
        // after normal or unwinding destruction of its value.
        unsafe { free_slot_local::<T>(self.slot) };
    }
}

/// Reclaims a slot from a pointer to its **value** (field 0 of `SlotCell<T>`),
/// for a possibly-unsized `T`, by reconstructing the slot and chunk layout from
/// the value's runtime size and alignment.
///
/// This erased path never names `SlotCell<T>` (which is illegal for unsized
/// `T`), so it can reclaim an unsized handle. For a `Sized` `T`,
/// `size_of_val`/`align_of_val` fold to the same constants the monomorphized
/// path uses, so the arithmetic collapses to the identical offsets.
///
/// # Safety
/// `value` must point at the initialized value of an occupied slot whose last
/// handle is being released; the value must not be accessed afterwards.
#[inline]
pub(crate) unsafe fn drop_and_free_val<T: ?Sized>(value: NonNull<T>) {
    // SAFETY: `value` refers to an occupied, initialized slot (caller contract).
    unsafe {
        // Read the pointer metadata (size/align) before running the destructor.
        let size = size_of_val(value.as_ref());
        let align = align_of_val(value.as_ref());
        if !needs_drop::<T>() {
            free_slot_erased(value.cast::<u8>(), size, align);
            return;
        }
        let guard = ErasedSlotGuard {
            value: value.cast::<u8>(),
            size,
            align,
        };
        drop_in_place(value.as_ptr());
        drop(guard);
    }
}

/// Pushes a freed slot back onto the free list and releases the pool refcount,
/// working purely from the value pointer plus the value's `size`/`align` — no
/// `SlotCell<T>` type needed. `PoolInner`/`ChunkHeader` layouts are independent
/// of the element type, so the erased `<()>` views recover the same addresses.
///
/// # Safety
/// `value` must point at field 0 of an occupied slot; `size`/`align` must be the
/// value's true size and alignment; the value must already have been dropped.
#[inline]
#[expect(
    clippy::cast_ptr_alignment,
    reason = "the reconstructed `SlotCell` fields sit at their natural alignments within the chunk allocation by construction"
)]
unsafe fn free_slot_erased(value: NonNull<u8>, size: usize, align: usize) {
    // Reconstruct the `#[repr(C)] SlotCell<T>` layout: `{ value, refcount: u32,
    // index: u32 }`, aligned to `max(align, align_of::<u32>())`.
    let refcount_align = align_of::<AtomicU32>();
    let refcount_size = size_of::<AtomicU32>();
    let index_align = align_of::<u32>();
    let index_size = size_of::<u32>();
    let cell_align = align.max(refcount_align).max(index_align);
    let refcount_off = round_up(size, refcount_align);
    let index_off = round_up(refcount_off + refcount_size, index_align);
    let stride = round_up(index_off + index_size, cell_align);
    let slots_off = round_up(size_of::<ChunkHeader>(), cell_align);

    // SAFETY: the addresses below are the same ones `header_of`/`push_free`
    // compute for the concrete `T`; see the layout reconstruction above.
    unsafe {
        let base = value.as_ptr();
        let index = base.add(index_off).cast::<u32>().read();
        let refcount = &*base.add(refcount_off).cast::<AtomicU32>();
        let header = &*base.sub(index as usize * stride + slots_off).cast::<ChunkHeader>();
        let pool = header.pool;
        let global = header.base_index + index;
        let inner = pool.as_ref();

        loop {
            let head = inner.free_head.load(Relaxed);
            refcount.store(head, Relaxed);
            if inner.free_head.compare_exchange_weak(head, global, Release, Relaxed).is_ok() {
                break;
            }
        }
        if inner.pool_refcount.fetch_sub(1, Release) == 1 {
            fence(Acquire);
            (inner.teardown)(pool);
        }
    }
}

/// Returns a raw pointer to the slot's refcount, given a pointer to its value.
///
/// The refcount sits at `round_up(size_of_val, align_of::<u32>())` within the
/// `#[repr(C)] SlotCell<T>` (the value is field 0). For a `Sized` `T` this folds
/// to a constant offset, matching the monomorphized field access.
///
/// # Safety
/// `value` must point at the (valid, occupied) value of a live slot.
#[inline]
#[expect(
    clippy::cast_ptr_alignment,
    reason = "the refcount follows the value at its natural `AtomicU32` alignment within the `#[repr(C)]` slot"
)]
pub(crate) unsafe fn refcount_ptr<T: ?Sized>(value: NonNull<T>) -> *mut AtomicU32 {
    // SAFETY: `value` is field 0 of the slot, so the refcount follows it.
    // `value.as_ref()` forms a `&T` to the slot's value, which the caller
    // guarantees is a valid, live value for the duration of this call;
    // `size_of_val` then reads only the pointer metadata (length or vtable),
    // not the value's bytes.
    unsafe {
        let refcount_off = round_up(size_of_val(value.as_ref()), align_of::<AtomicU32>());
        value.as_ptr().cast::<u8>().add(refcount_off).cast::<AtomicU32>()
    }
}

/// Drops and frees a lifetime-bound `Alloc`, returning the slot **without**
/// touching `pool_refcount`.
///
/// # Safety
/// `slot` must be an occupied, initialized slot whose `Alloc` handle is being
/// dropped; its value must not be accessed afterwards.
#[inline]
pub(crate) unsafe fn drop_and_free_local<T>(slot: NonNull<SlotCell<T>>) {
    // SAFETY: the `Alloc`'s owner is dropping an occupied slot (caller contract).
    unsafe {
        let guard = LocalSlotGuard { slot };
        SlotCell::drop_value(slot);
        drop(guard);
    }
}

/// Frees every chunk, the directory, and the `PoolInner` itself.
///
/// # Safety
/// The pool refcount must have just reached zero; the pool is quiescent.
#[cold]
#[inline(never)]
unsafe fn teardown<T, A: Allocator>(pool: NonNull<PoolInner<T, A>>) {
    // SAFETY: exclusive ownership; chunks were allocated with `chunk_layout`.
    unsafe {
        let inner = pool.as_ref();
        let layout = inner.chunk_layout;
        // Synchronize with `grow()`'s `chunks_allocated.store(Release)`, which is
        // sequenced after each `directory.push`. Without this acquire, a teardown
        // running on a thread that never observed `grow()` directly could read a
        // stale or partially published directory (its `pool_refcount` increments
        // are `Relaxed`, so they do not publish the directory on their own).
        let _ = inner.chunks_allocated.load(Acquire);
        {
            let dir = &*inner.directory.get();
            for &chunk in dir {
                // Loom's instrumented atomics must be dropped, not just freed,
                // or loom reports them leaked. A no-op (compiled out) otherwise.
                #[cfg(loom)]
                for i in 0..inner.chunk_size {
                    let slot = slot_at::<T>(chunk, i as usize);
                    core::ptr::drop_in_place(&raw mut (*slot.as_ptr()).refcount);
                }

                inner.allocator.deallocate(chunk.cast::<u8>(), layout);
            }
        }
        drop(AllocBox::from_raw(pool.as_ptr()));
    }
}

/// Restores a type-erased core pointer to its concrete pool type.
///
/// # Safety
/// `core` must be the first field of a live `PoolInner<T, A>` whose refcount
/// just reached zero.
pub(crate) unsafe fn teardown_erased<T, A: Allocator>(core: NonNull<PoolCore>) {
    // SAFETY: `PoolInner` is `#[repr(C)]` with `core` as its first field, and
    // this monomorphized callback was stored by that exact pool allocation.
    unsafe { teardown::<T, A>(core.cast::<PoolInner<T, A>>()) };
}

#[cold]
#[expect(clippy::panic, reason = "the panicking `alloc_*` methods document that they panic on exhaustion")]
#[inline(never)]
fn pool_full(err: AllocError) -> ! {
    panic!("plurality: {err}");
}
