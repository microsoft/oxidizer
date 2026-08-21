// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "pointer-recovery and slot-lifecycle paths group tightly-coupled unsafe operations under a single documented safety invariant; one block per operation would duplicate that invariant and obscure it"
)]

use alloc::boxed::Box as AllocBox;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::any::type_name;
use core::cell::UnsafeCell;
use core::fmt;
use core::mem::{MaybeUninit, needs_drop};
use core::pin::Pin;
use core::ptr::{NonNull, drop_in_place};

use allocator_api2::alloc::{Allocator, Global};

use crate::alloced::Alloc;
use crate::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use crate::atomic::{AtomicU32, AtomicUsize, fence};
use crate::boxed::Box;
use crate::builder::PoolBuilder;
use crate::chunk::ChunkHeader;
use crate::directory;
use crate::error::AllocError;
use crate::geometry::{self, RuntimeGeometry, SlotGeometry, TypedGeometry};
#[cfg(feature = "stats")]
use crate::pool_stats::PoolStats;
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
    /// Returns the core to its concrete `PoolInner<A, G>` type for teardown.
    pub(crate) teardown: unsafe fn(NonNull<Self>),
}

#[inline]
#[cfg_attr(test, mutants::skip)] // Differences occur only at the unallocatable u32 slot-index ceiling.
const fn unbounded_chunk_cap(chunk_size: u32) -> u64 {
    MAX_POOL_SLOTS / chunk_size as u64
}

/// Concrete pool state. `core` is first so its full-provenance pointer can be
/// cast back by the concrete teardown callback stored inside it.
///
/// Carries no element type: the geometry provider `G` supplies every layout
/// number the body needs, and — for a typed pool — the element-type marker.
/// Chunk memory is managed without ever reading or dropping value storage, so
/// the body has no use for the element type beyond its layout.
#[repr(C)]
pub(crate) struct PoolInner<A, G> {
    pub(crate) core: PoolCore,
    /// This pool's own address, captured from the pointer its allocation was
    /// created with.
    ///
    /// Every chunk header carries a copy so that the last handle to be dropped
    /// can free the pool. That pointer must be able to deallocate, so it cannot
    /// be derived from a `&self` borrow — such a pointer only permits reads and
    /// interior-mutable writes, and freeing through it is undefined behaviour.
    /// Ref: docs/implementation/pool-body.md, "Pointer recovery".
    pub(crate) me: NonNull<PoolCore>,
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
    /// Supplies the slot offsets and stride this pool's chunks are laid out to.
    pub(crate) geometry: G,
}

/// A growable, fixed-slot object pool.
///
/// See the [crate-level documentation](crate) for the concurrency model. The
/// pool is `Send` when the allocator `A` is — whatever `T` is — so it can be
/// moved between threads, but it is **not** `Sync` (only one thread allocates
/// at a time). It produces four handle types — `Box`, `Alloc`, `Arc`, `Rc`.
/// `Box` and `Arc` are `Send` (when `T` and the allocator `A` are), so they may
/// be dropped from any thread; `Alloc` and `Rc` are `!Send` and stay on the
/// allocating thread.
pub struct Pool<T, A: Allocator = Global> {
    inner: NonNull<PoolInner<A, TypedGeometry<T>>>,
}

// SAFETY: all cross-thread state in `PoolInner` is atomic; the non-atomic
// directory is only ever touched by the single allocator thread (guaranteed by
// `!Sync`) or at teardown when the pool is quiescent.
//
// There is deliberately no `T: Send` bound. A pool object owns no values:
// every safely reachable value is owned through a handle, and a handle that
// crosses threads carries its own `T: Send` requirement. The pool exposes no
// iteration or drain, so a receiving thread has no route to a value some other
// thread placed here; it can only obtain free slots, which hold no live value.
// Teardown deallocates chunks without ever reading or dropping element
// storage, so it cannot touch a `T` either.
//
// This argument only holds while the pool object stays value-free, so it binds
// future API: no method may yield or drop a pooled value through the pool, and
// the pool may not become `Sync`. Ref: docs/DESIGN.md, invariant 7 ("The pool
// object neither yields nor drops pooled values").
unsafe impl<T, A: Allocator + Send> Send for Pool<T, A> {}

// Pool state transitions leave the pool usable when they unwind. The allocator
// may also be shared with detached handles, so it must itself be safe through
// shared references.
impl<T, A: Allocator + core::panic::RefUnwindSafe> core::panic::RefUnwindSafe for Pool<T, A> {}
impl<T, A: Allocator + core::panic::RefUnwindSafe> core::panic::UnwindSafe for Pool<T, A> {}

impl<T, A: Allocator> fmt::Debug for Pool<T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
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
    pub(crate) fn from_inner(inner: NonNull<PoolInner<A, TypedGeometry<T>>>) -> Self {
        Self { inner }
    }

    #[inline]
    fn inner(&self) -> &PoolInner<A, TypedGeometry<T>> {
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
    pub fn stats(&self) -> PoolStats {
        let inner = self.inner();
        PoolStats {
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
    /// Panics if allocation fails. Use [`try_alloc_box`](Self::try_alloc_box)
    /// to handle capacity exhaustion and allocator failure.
    #[inline]
    pub fn alloc_box(&self, value: T) -> Box<T, A> {
        match self.try_alloc_box(value) {
            Ok(b) => b,
            Err(err) => allocation_failed(err),
        }
    }

    /// Allocates a value produced by `f` and returns a unique [`Box`]. `f` is
    /// not called if allocation fails.
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[inline]
    pub fn alloc_box_with<F: FnOnce() -> T>(&self, f: F) -> Box<T, A> {
        match self.try_alloc_box_with(f) {
            Ok(b) => b,
            Err(err) => allocation_failed(err),
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
    /// Panics if allocation fails.
    #[inline]
    pub fn alloc_arc(&self, value: T) -> Arc<T, A>
    where
        T: Send + Sync,
    {
        match self.try_alloc_arc(value) {
            Ok(a) => a,
            Err(err) => allocation_failed(err),
        }
    }

    /// Allocates a value produced by `f` and returns a shared [`Arc`].
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[inline]
    pub fn alloc_arc_with<F: FnOnce() -> T>(&self, f: F) -> Arc<T, A>
    where
        T: Send + Sync,
    {
        match self.try_alloc_arc_with(f) {
            Ok(a) => a,
            Err(err) => allocation_failed(err),
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

    /// Allocates `value` and returns a construction-time pinned shared [`Arc`].
    ///
    /// No ordinary `Arc` to the allocation is exposed before it is pinned,
    /// matching [`alloc::sync::Arc::pin`].
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[inline]
    pub fn alloc_arc_pin(&self, value: T) -> Pin<Arc<T, A>>
    where
        T: Send + Sync,
    {
        match self.try_alloc_arc_pin(value) {
            Ok(a) => a,
            Err(err) => allocation_failed(err),
        }
    }

    /// Allocates a value produced by `f` and returns a construction-time pinned
    /// shared [`Arc`].
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[inline]
    pub fn alloc_arc_pin_with<F: FnOnce() -> T>(&self, f: F) -> Pin<Arc<T, A>>
    where
        T: Send + Sync,
    {
        match self.try_alloc_arc_pin_with(f) {
            Ok(a) => a,
            Err(err) => allocation_failed(err),
        }
    }

    /// Fallible [`alloc_arc_pin`](Self::alloc_arc_pin).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `value` is dropped.
    #[inline]
    pub fn try_alloc_arc_pin(&self, value: T) -> Result<Pin<Arc<T, A>>, AllocError>
    where
        T: Send + Sync,
    {
        let fresh = self.try_alloc_arc(value)?;
        // SAFETY: `fresh` was just constructed here and no alias has escaped.
        Ok(unsafe { Arc::into_pin_fresh(fresh) })
    }

    /// Fallible [`alloc_arc_pin_with`](Self::alloc_arc_pin_with).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `f` is not called.
    #[inline]
    pub fn try_alloc_arc_pin_with<F: FnOnce() -> T>(&self, f: F) -> Result<Pin<Arc<T, A>>, AllocError>
    where
        T: Send + Sync,
    {
        let fresh = self.try_alloc_arc_with(f)?;
        // SAFETY: `fresh` was just constructed here and no alias has escaped.
        Ok(unsafe { Arc::into_pin_fresh(fresh) })
    }

    // ─── Alloc<'pool, T> (unique, lifetime-bound, cheapest) ──────────────

    /// Allocates `value` and returns an [`Alloc`] — a unique handle that borrows
    /// the pool. It cannot outlive the pool, but is the cheapest handle.
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[inline]
    pub fn alloc(&self, value: T) -> Alloc<'_, T, A> {
        match self.try_alloc(value) {
            Ok(a) => a,
            Err(err) => allocation_failed(err),
        }
    }

    /// Allocates a value produced by `f` and returns an [`Alloc`]. `f` is not
    /// called if allocation fails.
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[inline]
    pub fn alloc_with<F: FnOnce() -> T>(&self, f: F) -> Alloc<'_, T, A> {
        match self.try_alloc_with(f) {
            Ok(a) => a,
            Err(err) => allocation_failed(err),
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
                unsafe { occupy_local(slot, value) };
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
    /// Panics if allocation fails.
    #[inline]
    pub fn alloc_rc(&self, value: T) -> Rc<T, A> {
        match self.try_alloc_rc(value) {
            Ok(r) => r,
            Err(err) => allocation_failed(err),
        }
    }

    /// Allocates a value produced by `f` and returns an [`Rc`].
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[inline]
    pub fn alloc_rc_with<F: FnOnce() -> T>(&self, f: F) -> Rc<T, A> {
        match self.try_alloc_rc_with(f) {
            Ok(r) => r,
            Err(err) => allocation_failed(err),
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

    /// Allocates `value` and returns a construction-time pinned shared [`Rc`].
    ///
    /// No ordinary `Rc` to the allocation is exposed before it is pinned,
    /// matching [`alloc::rc::Rc::pin`].
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[inline]
    pub fn alloc_rc_pin(&self, value: T) -> Pin<Rc<T, A>> {
        match self.try_alloc_rc_pin(value) {
            Ok(r) => r,
            Err(err) => allocation_failed(err),
        }
    }

    /// Allocates a value produced by `f` and returns a construction-time pinned
    /// shared [`Rc`].
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[inline]
    pub fn alloc_rc_pin_with<F: FnOnce() -> T>(&self, f: F) -> Pin<Rc<T, A>> {
        match self.try_alloc_rc_pin_with(f) {
            Ok(r) => r,
            Err(err) => allocation_failed(err),
        }
    }

    /// Fallible [`alloc_rc_pin`](Self::alloc_rc_pin).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `value` is dropped.
    #[inline]
    pub fn try_alloc_rc_pin(&self, value: T) -> Result<Pin<Rc<T, A>>, AllocError> {
        let fresh = self.try_alloc_rc(value)?;
        // SAFETY: `fresh` was just constructed here and no alias has escaped.
        Ok(unsafe { Rc::into_pin_fresh(fresh) })
    }

    /// Fallible [`alloc_rc_pin_with`](Self::alloc_rc_pin_with).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `f` is not called.
    #[inline]
    pub fn try_alloc_rc_pin_with<F: FnOnce() -> T>(&self, f: F) -> Result<Pin<Rc<T, A>>, AllocError> {
        let fresh = self.try_alloc_rc_with(f)?;
        // SAFETY: `fresh` was just constructed here and no alias has escaped.
        Ok(unsafe { Rc::into_pin_fresh(fresh) })
    }

    // ─── uninitialized placement ─────────────────────────────────────────

    /// Reserves a slot and returns an uninitialized [`Box`], for placing a
    /// value directly into pool memory. Call
    /// [`assume_init`](crate::Box::assume_init) once written.
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_box(&self) -> Box<MaybeUninit<T>, A> {
        match self.try_alloc_uninit_box() {
            Ok(b) => b,
            Err(err) => allocation_failed(err),
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
    /// Panics if allocation fails.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_arc(&self) -> Arc<MaybeUninit<T>, A>
    where
        T: Send + Sync,
    {
        match self.try_alloc_uninit_arc() {
            Ok(a) => a,
            Err(err) => allocation_failed(err),
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
    /// Panics if allocation fails.
    #[must_use]
    #[inline]
    pub fn alloc_uninit(&self) -> Alloc<'_, MaybeUninit<T>, A> {
        match self.try_alloc_uninit() {
            Ok(a) => a,
            Err(err) => allocation_failed(err),
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
    /// Panics if allocation fails.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_rc(&self) -> Rc<MaybeUninit<T>, A> {
        match self.try_alloc_uninit_rc() {
            Ok(r) => r,
            Err(err) => allocation_failed(err),
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
        unsafe { self.inner().core.occupy(slot, value) };
    }

    /// Occupies a slot for a `Box` without initializing its unused slot
    /// refcount; `push_free` overwrites that field on drop.
    ///
    /// # Safety
    /// `slot` must have just been popped off the free list.
    #[inline]
    unsafe fn occupy_box(&self, slot: NonNull<SlotCell<T>>, value: T) {
        // SAFETY: exclusive ownership of the freshly popped slot.
        unsafe { self.inner().core.occupy_box(slot, value) };
    }

    /// Marks a freshly popped slot occupied (refcount = 1) and bumps the pool
    /// refcount, without writing a value. Used by the shared `Arc`/`Rc` paths.
    ///
    /// # Safety
    /// `slot` must have just been popped off the free list.
    #[inline]
    unsafe fn mark_occupied(&self, slot: NonNull<SlotCell<T>>) {
        // SAFETY: exclusive ownership of the freshly popped slot.
        unsafe { self.inner().core.mark_occupied(slot) };
    }

    /// Bumps the pool refcount for one new refcounted allocation
    /// (`Box`/`Arc`/`Rc`).
    #[inline]
    fn bump_pool_ref(&self) {
        self.inner().core.bump_pool_ref();
    }

    /// Pops a free slot, growing the pool if necessary. Returns `Err` only if
    /// allocation fails (see [`AllocError`] for the cause).
    #[inline]
    fn alloc_slot(&self) -> Result<NonNull<SlotCell<T>>, AllocError> {
        match self.inner().alloc_slot() {
            Ok(slot) => Ok(slot.cast::<SlotCell<T>>()),
            Err(err) => Err(err),
        }
    }
}

impl<A: Allocator, G: SlotGeometry> PoolInner<A, G> {
    /// Pops a free slot, growing the pool if necessary. Returns the slot's
    /// address, which is also its value's address. Returns `Err` only if the
    /// allocation fails (see [`AllocError`] for the cause).
    #[inline]
    pub(crate) fn alloc_slot(&self) -> Result<NonNull<u8>, AllocError> {
        if let Some(slot) = self.pop_free() {
            return Ok(slot);
        }
        self.alloc_slot_by_growing()
    }

    /// The empty-free-list half of [`alloc_slot`](Self::alloc_slot).
    ///
    /// Kept out of line so that the free-list pop stays the fall-through path
    /// and its result stays in a register. Inlining this here costs the hot
    /// path a branch and a spill/reload pair, because the second `pop_free`
    /// below merges with the first through a stack slot.
    #[cold]
    #[inline(never)]
    fn alloc_slot_by_growing(&self) -> Result<NonNull<u8>, AllocError> {
        // `grow` reserves and returns the first slot of the new chunk (or an
        // `AllocError` if the pool can't grow).
        match self.grow() {
            Ok(slot) => Ok(slot),
            // A nested allocation may have published a chunk while ours was in
            // flight, leaving slots free even though growth is no longer
            // possible. That allocation has returned by now, so one more look
            // at the free list settles whether anything is there to hand out.
            // Ref: docs/implementation/reentrancy.md, "Growth".
            Err(err) => self.pop_free().ok_or(err),
        }
    }

    /// Pops a free slot, or returns `None` if the free list is empty.
    #[inline]
    fn pop_free(&self) -> Option<NonNull<u8>> {
        let geometry = self.geometry;
        loop {
            let head = self.core.free_head.load(Acquire);
            if head == FREE_END {
                return None;
            }
            // SAFETY: `head` is a valid global index currently on the free list.
            let slot = unsafe { self.slot_for_global(head) };
            // SAFETY: a free slot's refcount field holds the next-free link.
            let next = unsafe { (*refcount_of(geometry, slot)).load(Relaxed) };
            if self.core.free_head.compare_exchange_weak(head, next, AcqRel, Acquire).is_ok() {
                return Some(slot);
            }
        }
    }

    /// Returns a pointer to a slot's reference count.
    ///
    /// # Safety
    /// `slot` must address a live slot of this pool.
    #[inline]
    unsafe fn refcount_at(&self, slot: NonNull<u8>) -> *mut AtomicU32 {
        // SAFETY: the caller guarantees `slot` belongs to this pool.
        unsafe { refcount_of(self.geometry, slot) }
    }

    /// Maps a global slot index to its slot pointer via the directory. Only
    /// called on the (single) allocator thread.
    ///
    /// # Safety
    /// `g` must be a valid global index for an allocated chunk.
    #[inline]
    unsafe fn slot_for_global(&self, g: u32) -> NonNull<u8> {
        let chunk_no = (g >> self.shift) as usize;
        let offset = (g & self.mask) as usize;
        // SAFETY: single-thread directory access; `chunk_no = g / chunk_size` is
        // `< chunks_allocated == directory.len()` for any valid free-list index.
        let chunk = unsafe {
            let dir = &*self.directory.get();
            *dir.get_unchecked(chunk_no)
        };
        // SAFETY: `offset < chunk_size`. The slot's value is field 0, so the
        // slot address and the value address coincide.
        unsafe { self.geometry.slot_at(chunk, offset) }
    }

    /// Hands an unpublished chunk back to the allocator, reporting `err`.
    ///
    /// # Safety
    /// `ptr` must be a chunk allocation of this pool that was never published
    /// to the directory.
    #[cold]
    unsafe fn discard_chunk<R>(&self, ptr: NonNull<ChunkHeader>, err: AllocError) -> Result<R, AllocError> {
        // SAFETY: the caller guarantees an unpublished chunk of this pool,
        // which is allocated with `chunk_layout`.
        unsafe { self.allocator.deallocate(ptr.cast::<u8>(), self.chunk_layout) };
        Err(err)
    }

    /// Allocates and installs one new chunk, reserves its first slot for the
    /// caller, and splices the rest onto the free list. Returns the reserved
    /// slot, or an [`AllocError`] identifying why the pool cannot grow (capacity
    /// limit vs. allocator failure). Runs only on the allocator thread.
    ///
    /// Growth tolerates the allocator re-entering the pool.
    /// Ref: docs/implementation/reentrancy.md, "Growth".
    #[cold]
    #[inline(never)]
    fn grow(&self) -> Result<NonNull<u8>, AllocError> {
        let n = self.chunk_size;
        // Cap = the user's `max_chunks`, or for an unbounded pool the chunk count
        // that keeps every global index below the `FREE_END` sentinel.
        let cap = self.max_chunks.map_or_else(|| unbounded_chunk_cap(n), u64::from);
        if u64::from(self.chunks_allocated.load(Relaxed)) >= cap {
            return Err(AllocError::CAPACITY_EXHAUSTED);
        }

        // Allocate before claiming anything. Control leaves the pool here, and
        // an allocator that allocates from this pool runs a nested `grow` to
        // completion; deriving the chunk's identity afterwards means it derives
        // it from a count that already includes whatever the nested call
        // published. Ref: docs/implementation/reentrancy.md.
        let ptr = match self.allocator.allocate(self.chunk_layout) {
            Ok(p) => p.cast::<ChunkHeader>(),
            Err(_) => return Err(AllocError::ALLOCATOR_FAILED),
        };

        // Reserve the directory slot the chunk will be published into. This is
        // the last point control leaves the pool, so the publication below
        // cannot allocate and cannot be overtaken. The displaced buffer is
        // freed when this function returns, after publication.
        // SAFETY: no directory borrow is live here, and `!Sync` confines this
        // path to one thread.
        let _displaced = match unsafe { directory::reserve_one(&self.directory) } {
            Ok(displaced) => displaced,
            // SAFETY: `ptr` is the fresh allocation above, never published.
            Err(err) => return unsafe { self.discard_chunk(ptr, err) },
        };

        // Re-read the count now that control is back, and re-check the cap a
        // nested `grow` may have consumed. Without this the pool would overshoot
        // by the reentry depth, and an unbounded pool could derive slot indices
        // that reach the `FREE_END` sentinel.
        let chunks = self.chunks_allocated.load(Relaxed);
        if u64::from(chunks) >= cap {
            // SAFETY: `ptr` is the fresh allocation above, never published.
            return unsafe { self.discard_chunk(ptr, AllocError::CAPACITY_EXHAUSTED) };
        }
        let base_index = chunks * n;

        // Mirrors the assertion in `MultiPool::install`: if the reservation
        // above ever stopped covering the push below, the push would reallocate
        // under a live `&mut` rather than fail loudly, and that is an aliasing
        // violation no test in the suite observes.
        debug_assert!(
            // SAFETY: no directory borrow is live here.
            unsafe { directory::has_room(&self.directory) },
            "the reservation above must leave room for this push"
        );

        // SAFETY: `ptr` is a fresh, exclusively owned allocation sized for one
        // chunk; the header and all slots are initialized before publishing.
        // Each slot links to `i + 1`; the last link and slot 0 are fixed up by
        // the splice and caller below. Value storage is deliberately left
        // uninitialized.
        unsafe {
            ptr.as_ptr().write(ChunkHeader {
                // Carries the provenance the pool allocation was created with,
                // so that a handle outliving the pool can free it.
                pool: self.me,
                base_index,
                chunk_index: chunks,
            });
            for i in 0..n {
                let slot = self.geometry.slot_at(ptr, i as usize);
                self.refcount_at(slot).write(AtomicU32::new(base_index + i + 1));
                self.index_at(slot).write(i);
            }
            // Pushes into the capacity reserved above, so it neither allocates
            // nor panics and needs no guard against a partial publication.
            (&mut *self.directory.get()).push(ptr);
        }
        self.chunks_allocated.store(chunks + 1, Release);
        // `Relaxed` suffices: the counter is only read via `stats()`, never to
        // establish a happens-before relationship.
        #[cfg(feature = "stats")]
        self.bytes_allocated.fetch_add(self.chunk_layout.size(), Relaxed);

        // Splice the free slots (base_index+1 .. base_index+n-1) onto the head;
        // slot `base_index` is returned to the caller.
        if n > 1 {
            // SAFETY: `ptr` chunk is live; its last slot is index n-1.
            let last = unsafe { self.geometry.slot_at(ptr, (n - 1) as usize) };
            // SAFETY: `last` is the new chunk's (still-private) final slot.
            unsafe {
                splice_chain(self.refcount_at(last), &self.core.free_head, base_index + 1);
            };
        }
        // SAFETY: slot 0 of the new chunk; never published, so exclusively ours.
        Ok(unsafe { self.geometry.slot_at(ptr, 0) })
    }

    /// Returns a pointer to a slot's in-chunk index.
    ///
    /// # Safety
    /// `slot` must address a live slot of this pool.
    #[inline]
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "the index sits at its natural `u32` alignment within the `#[repr(C)]` slot"
    )]
    unsafe fn index_at(&self, slot: NonNull<u8>) -> *mut u32 {
        // SAFETY: the index follows the refcount within the slot.
        unsafe { slot.as_ptr().add(self.geometry.index_offset()).cast::<u32>() }
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
/// `last_link` must be a valid, properly aligned pointer to the reference-count
/// link field of the final slot of a fully-initialized, not-yet-published chunk,
/// and must remain valid for the duration of the call. `base_index` must be the
/// global slot index at which that chunk's free chain begins.
#[cfg_attr(coverage_nightly, coverage(off))]
unsafe fn splice_chain(last_link: *mut AtomicU32, free_head: &AtomicU32, base_index: u32) {
    loop {
        let head = free_head.load(Acquire);
        // SAFETY: the new chain is private until the CAS publishes it.
        unsafe { (*last_link).store(head, Relaxed) };
        if free_head.compare_exchange_weak(head, base_index, AcqRel, Acquire).is_ok() {
            break;
        }
    }
}

/// Returns a raw pointer to a slot's refcount, given the slot's address.
///
/// # Safety
/// `slot` must address a live slot laid out by `geometry`.
#[inline]
#[expect(
    clippy::cast_ptr_alignment,
    reason = "the refcount sits at its natural `AtomicU32` alignment within the `#[repr(C)]` slot"
)]
unsafe fn refcount_of<G: SlotGeometry>(geometry: G, slot: NonNull<u8>) -> *mut AtomicU32 {
    // SAFETY: the refcount follows the value within the slot.
    unsafe { slot.as_ptr().add(geometry.refcount_offset()).cast::<AtomicU32>() }
}

impl<T, A: Allocator> Drop for Pool<T, A> {
    fn drop(&mut self) {
        let inner = self.inner();
        if inner.core.pool_refcount.fetch_sub(1, Release) == 1 {
            fence(Acquire);
            // SAFETY: a zero refcount grants exclusive ownership of the inner.
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
        let header = TypedGeometry::<T>::new().header_of(slot.cast::<u8>(), index);
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
/// `SlotCell<T>` type needed. A [`RuntimeGeometry`] built from the value's exact
/// layout derives the slot metadata offsets and the chunk-header address,
/// yielding the same locations the allocating pool's own geometry evaluates.
/// Ref: docs/implementation/geometry.md.
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
    let geometry = RuntimeGeometry::new(
        // SAFETY: the caller guarantees these are a live value's true size and
        // alignment, which therefore already satisfy the `Layout` invariants.
        unsafe { Layout::from_size_align_unchecked(size, align) },
    );

    // SAFETY: the addresses below come from the same formulas the geometry
    // evaluates for the concrete `T`, over the same size and alignment, so they
    // resolve to the same locations.
    unsafe {
        let base = value.as_ptr();
        let index = base.add(geometry.index_offset()).cast::<u32>().read();
        let refcount = &*base.add(geometry.refcount_offset()).cast::<AtomicU32>();
        let header = geometry.header_of(value, index).as_ref();
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
/// The refcount sits at the geometry's refcount offset within the
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
        let refcount_off = geometry::refcount_offset(size_of_val(value.as_ref()));
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
pub(crate) unsafe fn teardown<A: Allocator, G: SlotGeometry>(pool: NonNull<PoolInner<A, G>>) {
    // SAFETY: exclusive ownership; chunks were allocated with `chunk_layout`.
    unsafe {
        let inner = pool.as_ref();
        let layout = inner.chunk_layout;
        // Acquire the directory publication from `grow`; relaxed pool-refcount
        // increments do not make it visible to a different teardown thread.
        let _ = inner.chunks_allocated.load(Acquire);
        // Re-borrowed per chunk so that no directory borrow is live across the
        // `deallocate` call below, keeping the crate's rule that control never
        // leaves the pool while a directory is borrowed. The length is sampled
        // once rather than re-read per iteration, as `grow` does: teardown runs
        // at refcount zero, so no handle survives through which an allocator
        // could re-enter and publish a chunk.
        // Ref: docs/implementation/reentrancy.md.
        let chunks = (&*inner.directory.get()).len();
        for i in 0..chunks {
            let chunk = (&*inner.directory.get())[i];
            // Loom's instrumented atomics must be dropped, not just freed,
            // or loom reports them leaked. A no-op (compiled out) otherwise.
            #[cfg(loom)]
            for i in 0..inner.chunk_size {
                let slot = inner.geometry.slot_at(chunk, i as usize);
                drop_in_place(refcount_of(inner.geometry, slot));
            }

            inner.allocator.deallocate(chunk.cast::<u8>(), layout);
        }
        drop(AllocBox::from_raw(pool.as_ptr()));
    }
}

/// Restores a type-erased core pointer to its concrete pool type.
///
/// # Safety
/// `core` must be the first field of a live `PoolInner<A, G>` whose refcount
/// just reached zero.
pub(crate) unsafe fn teardown_erased<A: Allocator, G: SlotGeometry>(core: NonNull<PoolCore>) {
    // SAFETY: `PoolInner` is `#[repr(C)]` with `core` as its first field, and
    // this monomorphized callback was stored by that exact pool allocation.
    unsafe { teardown::<A, G>(core.cast::<PoolInner<A, G>>()) };
}

/// Records a freshly allocated pool's own address in its metadata.
///
/// # Safety
/// `raw` must address an initialized `PoolInner` that nothing has borrowed yet,
/// and must carry provenance over the whole pool allocation — it is the pointer
/// teardown eventually frees.
pub(crate) unsafe fn publish_address<A: Allocator, G: SlotGeometry>(raw: NonNull<PoolInner<A, G>>) {
    // SAFETY: the caller guarantees an initialized, unshared pool allocation.
    // `PoolInner` is `#[repr(C)]` with `core` first, so the cast addresses the
    // core without changing the pointer's provenance.
    unsafe { (*raw.as_ptr()).me = raw.cast::<PoolCore>() };
}

impl PoolCore {
    /// Writes `value` into a freshly popped slot, marks it occupied, and bumps
    /// the pool refcount.
    ///
    /// # Safety
    /// `slot` must have just been popped off this pool's free list (no other
    /// reference to it exists) and must be laid out for `T`.
    #[inline]
    pub(crate) unsafe fn occupy<T>(&self, slot: NonNull<SlotCell<T>>, value: T) {
        // SAFETY: exclusive ownership of the freshly popped slot.
        unsafe {
            self.mark_occupied(slot);
            SlotCell::write_value(slot, value);
        }
    }

    /// Occupies a slot for a `Box` without initializing its unused slot
    /// refcount; `push_free` overwrites that field on drop.
    ///
    /// # Safety
    /// As for [`occupy`](Self::occupy).
    #[inline]
    pub(crate) unsafe fn occupy_box<T>(&self, slot: NonNull<SlotCell<T>>, value: T) {
        self.bump_pool_ref();
        // SAFETY: exclusive ownership of the freshly popped slot.
        unsafe { SlotCell::write_value(slot, value) };
    }

    /// Marks a freshly popped slot occupied (refcount = 1) and bumps the pool
    /// refcount, without writing a value. Used by the shared `Arc`/`Rc` paths.
    ///
    /// # Safety
    /// As for [`occupy`](Self::occupy).
    #[inline]
    pub(crate) unsafe fn mark_occupied<T>(&self, slot: NonNull<SlotCell<T>>) {
        // SAFETY: exclusive ownership of the freshly popped slot.
        unsafe { (*slot.as_ptr()).refcount.store(1, Relaxed) };
        self.bump_pool_ref();
    }

    /// Bumps the pool refcount for one new refcounted allocation
    /// (`Box`/`Arc`/`Rc`).
    #[inline]
    pub(crate) fn bump_pool_ref(&self) {
        let _ = self.pool_refcount.fetch_add(1, Relaxed);
    }
}

/// Occupies a slot for an `Alloc` without touching either refcount. Its borrow
/// keeps the pool alive, and `push_free` overwrites the unused slot refcount on
/// drop.
///
/// # Safety
/// `slot` must have just been popped off a pool's free list and be laid out
/// for `T`.
#[inline]
pub(crate) unsafe fn occupy_local<T>(slot: NonNull<SlotCell<T>>, value: T) {
    // SAFETY: exclusive ownership of the freshly popped slot.
    unsafe { SlotCell::write_value(slot, value) };
}

#[cold]
#[expect(
    clippy::panic,
    reason = "the panicking `alloc_*` methods document that they panic when allocation fails"
)]
#[inline(never)]
pub(crate) fn allocation_failed(err: AllocError) -> ! {
    panic!("plurality: {err}");
}
