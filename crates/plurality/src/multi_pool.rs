// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A pool for values of any type.
//!
//! The router that maps a value's layout to the [`LayoutPool`] serving it, and
//! the allocation surface layered on top of it. See
//! `docs/design/multi-pool.md` for the user-visible model and
//! `docs/implementation/multi-pool.md` for the reentrancy ordering the cold
//! path follows.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "the routing and installation paths take several borrows of the two parallel vectors under one precondition, that the caller holds the single-threaded allocation path, and rely on the push order keeping the vectors index-aligned; one block per borrow would repeat both at every site"
)]

use alloc::vec::Vec;
use core::alloc::Layout;
use core::any::type_name;
use core::cell::UnsafeCell;
use core::fmt;
use core::mem::MaybeUninit;
use core::pin::Pin;

use allocator_api2::alloc::{Allocator, Global};

use crate::alloced::Alloc;
use crate::boxed::Box;
use crate::directory::{self, Displaced};
use crate::error::AllocError;
use crate::layout_pool::{LayoutPool, LayoutPoolRef};
use crate::multi_builder::MultiPoolBuilder;
use crate::pool::{allocation_failed, occupy_local};
use crate::rc::Rc;
use crate::slot::{MAX_CHUNK_SIZE_SLOTS, SlotCell};
use crate::sync::Arc;

/// Chunk sizing shared by every layout pool a [`MultiPool`] creates.
///
/// A byte target lets layouts of very different sizes commit comparable memory
/// per growth step; a fixed slot count gives every layout equal increments of
/// capacity. Ref: docs/design/multi-pool.md, "Sizing chunks by bytes".
#[derive(Clone, Copy, Debug)]
pub(crate) enum ChunkSizing {
    /// Each layout pool takes as many slots as fit in this many bytes.
    Bytes(usize),
    /// Every layout pool starts from this slot count.
    Slots(u32),
}

impl ChunkSizing {
    /// Slots per chunk for a layout of `stride` bytes, before the layout pool
    /// applies its own overflow clamp.
    ///
    /// A chunk always holds at least one slot, so a value larger than the byte
    /// target on its own gets a chunk sized by the value.
    /// Ref: docs/design/multi-pool.md, "Bounding growth".
    fn slots_for(self, stride: usize) -> u32 {
        match self {
            Self::Bytes(target) => {
                let slots = target / stride.max(1);
                let slots = u32::try_from(slots).unwrap_or(u32::MAX).clamp(1, MAX_CHUNK_SIZE_SLOTS);
                // Rounding down keeps the chunk within the byte target. Chunk
                // sizes stay powers of two so that slot addressing remains
                // shift-and-mask arithmetic.
                1 << (u32::BITS - 1 - slots.leading_zeros())
            }
            Self::Slots(slots) => slots,
        }
    }
}

/// An object pool that accepts values of any type.
///
/// One pool object backs a heterogeneous working set: the type parameter
/// travels with the allocation rather than with the pool. Values are routed to
/// the internal pool serving their slot geometry, so each occupies the same
/// space it would in a pool dedicated to that one type — sizes are never
/// rounded up to share a size class.
/// Ref: docs/design/multi-pool.md, "Exact sizes, no size classes".
///
/// Allocation hands back an owned or shared handle, either detachable or bound
/// to the pool's borrow. Each keeps its value at a stable address for as
/// long as it lives and is one pointer wide for a sized value; the detachable
/// handles also coerce to trait objects and slices. Freeing costs no more than
/// it would from a pool dedicated to that one type, because a handle finds its
/// own pool by pointer recovery and never consults the router.
///
/// A multi pool is `Send` when its allocator is, and is not `Sync`: one thread
/// allocates at a time, while frees may happen anywhere. Values of types with
/// different thread affinities may share one pool, because the pool owns no
/// values — each handle carries its own bound.
///
/// # Exhaustion and allocator failure
///
/// Capacity exhaustion covers two cases: the layout pool serving the request
/// cannot grow further, or the request is for an unseen layout and the pool
/// already holds its maximum number of layouts. Allocator failure additionally
/// covers the metadata of a layout pool created on first sight of a layout. The
/// `alloc_*` methods panic in either case; the `try_alloc_*` methods report an
/// [`AllocError`]. "Full" below always means capacity exhaustion alone;
/// "allocation failed" covers either cause.
///
/// ```
/// use plurality::MultiPool;
///
/// let pool = MultiPool::new();
/// let widget = pool.alloc_box(42_u64);
/// let name = pool.alloc_box(String::from("hello"));
/// assert_eq!(*widget, 42);
/// assert_eq!(&*name, "hello");
/// assert_eq!(pool.layouts(), 2);
/// ```
pub struct MultiPool<A: Allocator + Clone = Global> {
    /// Keys, in first-seen order. Kept in their own vector so a lookup scans
    /// keys only, without striding over pool pointers it does not need.
    layouts: UnsafeCell<Vec<Layout>>,
    /// `pools[i]` serves `layouts[i]`. Never shorter than `layouts`, so a key
    /// is never visible without the pool that serves it.
    pools: UnsafeCell<Vec<LayoutPool<A>>>,
    sizing: ChunkSizing,
    max_chunks: Option<u32>,
    max_layouts: Option<usize>,
    allocator: A,
}

// SAFETY: all cross-thread state is atomic; the two directory vectors are
// touched only on the allocation path, which `!Sync` confines to one thread at
// a time; and the pool object owns no values, so a thread that receives one has
// no route to a value another thread placed in it — it can only draw free
// slots, which hold nothing live. Thread mobility for values is carried by the
// handles, each of which imposes its own bound.
// Ref: docs/DESIGN.md, invariant 7.
unsafe impl<A: Allocator + Clone + Send> Send for MultiPool<A> {}

impl<A: Allocator + Clone + core::panic::RefUnwindSafe> core::panic::RefUnwindSafe for MultiPool<A> {}
impl<A: Allocator + Clone + core::panic::RefUnwindSafe> core::panic::UnwindSafe for MultiPool<A> {}

impl MultiPool<Global> {
    /// Creates a multi pool with the default chunk sizing and unbounded growth.
    #[must_use]
    pub fn new() -> Self {
        MultiPoolBuilder::new().build()
    }

    /// Starts a [`MultiPoolBuilder`].
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Replacing the builder with Default is an unviable/equivalent mutant.
    pub fn builder() -> MultiPoolBuilder<Global> {
        MultiPoolBuilder::new()
    }
}

impl Default for MultiPool<Global> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Allocator + Clone> fmt::Debug for MultiPool<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("layouts", &self.layouts())
            .field("max_layouts", &self.max_layouts)
            .field("chunks_allocated", &self.chunks_allocated())
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

impl<A: Allocator + Clone> MultiPool<A> {
    pub(crate) fn from_parts(sizing: ChunkSizing, max_chunks: Option<u32>, max_layouts: Option<usize>, allocator: A) -> Self {
        Self {
            layouts: UnsafeCell::new(Vec::new()),
            pools: UnsafeCell::new(Vec::new()),
            sizing,
            max_chunks,
            max_layouts,
            allocator,
        }
    }

    /// Returns a view of the pool serving `T`, creating it on first sight.
    ///
    /// The returned view borrows nothing: the caller allocates through it after
    /// every directory borrow has been released, so reentrant user code is free
    /// to grow the directory in the meantime.
    #[inline]
    fn pool_for<T>(&self) -> Result<LayoutPoolRef<A>, AllocError> {
        // Step 1: scan for an existing entry.
        let layout = crate::geometry::routing_key(Layout::new::<T>());
        match self.lookup(layout) {
            Some(found) => Ok(found),
            None => self.install(layout),
        }
    }

    /// Creates the pool serving `layout` and installs it in the directory.
    ///
    /// `layout` is a routing key, not a value layout.
    ///
    /// Outlined and kept off the generic parameter so that an allocation whose
    /// layout is already known costs a scan and nothing else, and so that the
    /// directory-growth code is emitted once per allocator rather than once per
    /// element type.
    ///
    /// The step ordering below is load-bearing and is derived in
    /// `docs/implementation/multi-pool.md`, "Reentrancy". Every step that
    /// releases control to code outside this module is marked.
    #[cold]
    #[inline(never)]
    fn install(&self, layout: Layout) -> Result<LayoutPoolRef<A>, AllocError> {
        // Step 2: a miss against a reached layout cap is capacity exhaustion.
        // No borrow is held here, so the caller is free to drop a rejected
        // value — reentrant code — on the way out.
        if self.at_layout_cap() {
            return Err(AllocError::CAPACITY_EXHAUSTED);
        }

        // Step 3: build the pool. Fallible, touches no directory state, and
        // releases control to `A::clone` and to the global allocator, so a
        // reentrant allocation reaching either sees a consistent — merely
        // incomplete — directory.
        let stride = crate::geometry::stride(layout.size(), layout.align());
        let pool = LayoutPool::new(layout, self.sizing.slots_for(stride), self.max_chunks, self.allocator.clone())?;

        // Step 4: reserve after construction, so the reservation cannot be
        // consumed by a reentrant miss that happened during step 3. The
        // reservation is confirmed against both vectors before it is handed
        // back, and the displaced buffers are freed when this function returns
        // — after the pushes on the success path, and with nothing published on
        // an abandon or a cap return, so in neither case does an allocator call
        // separate a reservation from the push it guarantees.
        let _displaced = self.try_reserve_one()?;

        // Step 5: re-scan and re-check the cap. Step 3 released control twice.
        // Without the re-scan one layout could acquire two pools; without the
        // cap re-check, reentrant misses would overshoot the cap by their
        // depth.
        if let Some(found) = self.lookup(layout) {
            // Step 6: abandon without pushing. The borrow is already released,
            // so dropping `pool` — which runs the cloned allocator's destructor
            // and a global deallocation — is safe here.
            return Ok(found);
        }
        if self.at_layout_cap() {
            return Err(AllocError::CAPACITY_EXHAUSTED);
        }

        // Step 7: push `pools` first, then `layouts`, so `layouts.len() <=
        // pools.len()` holds at every instant and a key is never visible before
        // its pool. Both push into capacity reserved in step 4, so neither
        // reallocates and neither can fail.
        debug_assert!(
            // SAFETY: as for `lookup`.
            unsafe { directory::has_room(&self.pools) && directory::has_room(&self.layouts) },
            "step 4 must leave room in both directories"
        );

        // SAFETY: `!Sync` confines the allocation path to one thread, and no
        // borrow of either vector outlives this block. The two vectors live in
        // distinct cells, so the borrows taken here do not alias.
        let view = unsafe {
            let pools = &mut *self.pools.get();
            pools.push(pool);
            let view = pools[pools.len() - 1].as_ref();
            (*self.layouts.get()).push(layout);
            view
        };
        Ok(view)
    }

    /// Scans the key vector for `layout`, returning a view of its pool.
    ///
    /// Releases both borrows before returning, so the caller may run reentrant
    /// code immediately.
    #[inline]
    fn lookup(&self, layout: Layout) -> Option<LayoutPoolRef<A>> {
        // SAFETY: `!Sync` confines directory access to the allocation path on
        // one thread; the borrows end with this block.
        unsafe {
            let layouts = &*self.layouts.get();
            let index = layouts.iter().position(|&candidate| candidate == layout)?;
            // `layouts.len() <= pools.len()` is an invariant of the push order.
            let pools = &*self.pools.get();
            Some(pools[index].as_ref())
        }
    }

    /// `true` if a further layout would exceed the configured cap.
    #[inline]
    fn at_layout_cap(&self) -> bool {
        // SAFETY: as for `lookup`.
        let live = unsafe { (*self.layouts.get()).len() };
        self.max_layouts.is_some_and(|max| live >= max)
    }

    /// Reserves room for one more entry in both vectors.
    ///
    /// The displaced buffers are returned rather than freed, so that no
    /// allocator call separates the reservation from the pushes it guarantees.
    /// A reentrant miss could otherwise consume the reserved room and force the
    /// infallible push to reallocate.
    ///
    /// Reserving the second vector is itself such a call, so the room reserved
    /// in the first is confirmed once both are in hand rather than assumed.
    /// Ref: docs/implementation/reentrancy.md, "Reserving two vectors at once".
    fn try_reserve_one(&self) -> Result<(Displaced<LayoutPool<A>>, Displaced<Layout>), AllocError> {
        let mut previous = None;
        loop {
            // SAFETY: as for `lookup`; neither call holds a borrow across the
            // allocation it makes.
            let displaced = unsafe { (directory::reserve_one(&self.pools)?, directory::reserve_one(&self.layouts)?) };

            // SAFETY: as for `lookup`.
            let (installed, reserved) = unsafe {
                (
                    (*self.layouts.get()).len(),
                    directory::has_room(&self.pools) && directory::has_room(&self.layouts),
                )
            };
            if reserved {
                return Ok(displaced);
            }

            // Reserved room is only ever consumed by a reentrant install, which
            // consumes it by publishing a layout of its own. The installed
            // count is therefore strictly increasing across iterations, and
            // every install costs a layout pool's worth of memory, so the loop
            // terminates.
            debug_assert!(
                previous.is_none_or(|seen| installed > seen),
                "reservation retried without an intervening install"
            );
            previous = Some(installed);

            // Freeing the buffers displaced by this attempt is another point
            // where control leaves the pool, so it happens here, before the
            // next attempt re-reserves and re-checks.
            drop(displaced);
        }
    }

    /// Runs `f` over the pool serving `T`, creating it on first sight.
    ///
    /// Every allocation entry point funnels through here, so the router's
    /// ordering discipline is stated once.
    ///
    /// Inlined unconditionally: left to its own judgment the compiler emits
    /// this helper out of line, and the resulting call frame, argument setup,
    /// `Result` returned through memory and second copy of the payload into the
    /// closure's slot cost more than everything the helper does. The typed path
    /// does not route and is unaffected. The price is code size, since the
    /// directory scan is replicated per entry point per element type.
    /// Ref: docs/implementation/performance.md, "Attributing the routing cost".
    #[expect(
        clippy::inline_always,
        reason = "measured: out of line, the call costs more than the routing it wraps"
    )]
    #[inline(always)]
    fn with_pool<T, R>(&self, f: impl FnOnce(LayoutPoolRef<A>) -> Result<R, AllocError>) -> Result<R, AllocError> {
        // Step 8: allocate through a view that borrows nothing.
        f(self.pool_for::<T>()?)
    }

    // ─── introspection ───────────────────────────────────────────────────

    /// Number of internal layout pools the pool has created.
    ///
    /// Types of identical size and alignment share one layout pool, as do types
    /// whose alignments differ only below the width of the slot metadata.
    /// Ref: docs/design/multi-pool.md, "Exact sizes, no size classes".
    #[must_use]
    pub fn layouts(&self) -> usize {
        // SAFETY: as for `lookup`.
        unsafe { (*self.layouts.get()).len() }
    }

    /// The cap on internal layout pools, if any.
    #[must_use]
    pub fn max_layouts(&self) -> Option<usize> {
        self.max_layouts
    }

    /// Runs `f` over every layout pool, summing the results.
    ///
    /// Each view is copied out before `f` runs, so no directory borrow is live
    /// while `f` executes. Aggregate queries cost time proportional to the
    /// number of layout pools.
    #[inline]
    fn sum_pools(&self, f: fn(LayoutPoolRef<A>) -> u64) -> u64 {
        // SAFETY: as for `lookup`.
        let installed = unsafe { (*self.pools.get()).len() };
        let mut total = 0;
        for index in 0..installed {
            // SAFETY: as for `lookup`; `index` stays below the length read
            // above because none of the functions passed as `f` reach the pool:
            // each reads a counter out of the layout pool view it is handed,
            // and `f` being a plain `fn` closes that set to what this module
            // passes. That matters because a reservation leaves the vector
            // momentarily empty while it moves the elements into a larger
            // buffer. Ref: docs/implementation/reentrancy.md.
            // The view outlives the borrow it was copied out of.
            let view = unsafe { (&*self.pools.get())[index].as_ref() };
            total += f(view);
        }
        total
    }

    /// Live refcounted allocations (`Box`/`Arc`/`Rc`) across every layout.
    ///
    /// Lifetime-bound [`Alloc`](crate::Alloc) handles are **not** counted. A
    /// sum of independently read counters may describe a state the pool was
    /// never in; this is a reporting instrument, not a control-flow input.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.sum_pools(LayoutPoolRef::len)
    }

    /// `true` if no refcounted allocation is live (see [`len`](Self::len)).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Chunks held across every layout.
    #[must_use]
    pub fn chunks_allocated(&self) -> u64 {
        self.sum_pools(|pool| u64::from(pool.chunks_allocated()))
    }

    /// Total slots across every allocated chunk of every layout.
    #[must_use]
    pub fn capacity(&self) -> u64 {
        self.sum_pools(|pool| u64::from(pool.chunks_allocated()) * u64::from(pool.chunk_size()))
    }

    /// Effective slots per chunk in force for `T`'s layout.
    ///
    /// Reports the value the pool will actually use, after clamping. Never
    /// creates a layout pool. Zero for a layout too large to pool at all.
    #[must_use]
    pub fn chunk_size_of<T>(&self) -> u32 {
        self.with_layout_of::<T, _>(LayoutPoolRef::chunk_size, || {
            let layout = crate::geometry::routing_key(Layout::new::<T>());
            let stride = crate::geometry::stride(layout.size(), layout.align());
            crate::layout_pool::effective_chunk_size(layout, self.sizing.slots_for(stride))
        })
    }

    /// Effective chunk cap in force for `T`'s layout, after clamping. Zero for
    /// a layout too large to pool at all.
    #[must_use]
    pub fn max_chunks_of<T>(&self) -> u32 {
        self.with_layout_of::<T, _>(LayoutPoolRef::max_chunks, || {
            crate::layout_pool::effective_max_chunks(self.chunk_size_of::<T>(), self.max_chunks)
        })
    }

    /// Chunks held for `T`'s layout. Zero for an unseen layout.
    #[must_use]
    pub fn chunks_allocated_of<T>(&self) -> u32 {
        self.with_layout_of::<T, _>(LayoutPoolRef::chunks_allocated, || 0)
    }

    /// Live refcounted allocations of `T`'s layout. Zero for an unseen layout.
    #[must_use]
    pub fn len_of<T>(&self) -> u64 {
        self.with_layout_of::<T, _>(LayoutPoolRef::len, || 0)
    }

    /// Total slots across allocated chunks of `T`'s layout.
    #[must_use]
    pub fn capacity_of<T>(&self) -> u64 {
        self.with_layout_of::<T, _>(|pool| u64::from(pool.chunks_allocated()) * u64::from(pool.chunk_size()), || 0)
    }

    /// Lifetime totals summed across every layout pool.
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "stats")] {
    /// use plurality::MultiPool;
    ///
    /// let pool = MultiPool::new();
    /// assert_eq!(pool.stats().total_chunks_allocated, 0);
    ///
    /// let _a = pool.alloc_box(7_u64);
    /// let _b = pool.alloc_box([0_u8; 128]);
    /// let stats = pool.stats();
    /// assert_eq!(stats.total_chunks_allocated, 2);
    /// assert!(stats.total_bytes_allocated > 0);
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "stats")]
    #[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
    #[must_use]
    pub fn stats(&self) -> crate::PoolStats {
        crate::PoolStats {
            total_chunks_allocated: self.sum_pools(|pool| u64::from(pool.chunks_allocated())),
            total_bytes_allocated: self.sum_pools(LayoutPoolRef::bytes_allocated),
        }
    }

    /// Applies `found` to the pool serving `T`, or evaluates `absent` when the
    /// layout has not been seen. Never creates a layout pool.
    ///
    /// The view is copied out before `found` runs, so neither closure executes
    /// while a directory borrow is live.
    #[inline]
    fn with_layout_of<T, R>(&self, found: impl FnOnce(LayoutPoolRef<A>) -> R, absent: impl FnOnce() -> R) -> R {
        let layout = crate::geometry::routing_key(Layout::new::<T>());
        match self.lookup(layout) {
            Some(view) => found(view),
            None => absent(),
        }
    }

    // ─── Box<T> (unique owner) ───────────────────────────────────────────
    //
    // Every entry point below moves its value — or its construction closure —
    // into the routing closure, so a request the router rejects drops what it
    // was given only after every directory borrow has been released. Dropping
    // it may itself allocate from this pool.

    /// Allocates `value` and returns a unique [`Box`].
    ///
    /// # Panics
    /// Panics if allocation fails. Use [`try_alloc_box`](Self::try_alloc_box)
    /// to handle capacity exhaustion and allocator failure.
    ///
    /// ```
    /// use plurality::MultiPool;
    ///
    /// let pool = MultiPool::new();
    /// let value = pool.alloc_box([1_u8, 2, 3]);
    /// assert_eq!(*value, [1, 2, 3]);
    /// ```
    #[inline]
    pub fn alloc_box<T>(&self, value: T) -> Box<T, A> {
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
    pub fn alloc_box_with<T, F: FnOnce() -> T>(&self, f: F) -> Box<T, A> {
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
    pub fn try_alloc_box<T>(&self, value: T) -> Result<Box<T, A>, AllocError> {
        self.with_pool::<T, _>(|pool| {
            // SAFETY: the router selected `pool` for `Layout::new::<T>()`,
            // which is `alloc_slot`'s precondition.
            let slot = unsafe { pool.alloc_slot::<T>() }?;
            // SAFETY: `slot` was just popped and is owned exclusively here.
            unsafe { pool.core().occupy_box(slot, value) };
            Ok(Box::from_slot(slot))
        })
    }

    /// Fallible [`alloc_box_with`](Self::alloc_box_with).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `f` is not called.
    #[inline]
    pub fn try_alloc_box_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Box<T, A>, AllocError> {
        let mut uninit = self.try_alloc_uninit_box::<T>()?;
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
    pub fn alloc_arc<T>(&self, value: T) -> Arc<T, A>
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
    pub fn alloc_arc_with<T, F: FnOnce() -> T>(&self, f: F) -> Arc<T, A>
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
    pub fn try_alloc_arc<T>(&self, value: T) -> Result<Arc<T, A>, AllocError>
    where
        T: Send + Sync,
    {
        self.with_pool::<T, _>(|pool| {
            // SAFETY: the router selected `pool` for `Layout::new::<T>()`,
            // which is `alloc_slot`'s precondition.
            let slot = unsafe { pool.alloc_slot::<T>() }?;
            // SAFETY: `slot` was just popped and is owned exclusively here.
            unsafe { pool.core().occupy(slot, value) };
            Ok(Arc::from_slot(slot))
        })
    }

    /// Fallible [`alloc_arc_with`](Self::alloc_arc_with).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `f` is not called.
    #[inline]
    pub fn try_alloc_arc_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Arc<T, A>, AllocError>
    where
        T: Send + Sync,
    {
        let mut uninit = self.try_alloc_uninit_arc::<T>()?;
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
    pub fn alloc_arc_pin<T>(&self, value: T) -> Pin<Arc<T, A>>
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
    pub fn alloc_arc_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> Pin<Arc<T, A>>
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
    pub fn try_alloc_arc_pin<T>(&self, value: T) -> Result<Pin<Arc<T, A>>, AllocError>
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
    pub fn try_alloc_arc_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Pin<Arc<T, A>>, AllocError>
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
    pub fn alloc<T>(&self, value: T) -> Alloc<'_, T, A> {
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
    pub fn alloc_with<T, F: FnOnce() -> T>(&self, f: F) -> Alloc<'_, T, A> {
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
    pub fn try_alloc<T>(&self, value: T) -> Result<Alloc<'_, T, A>, AllocError> {
        self.with_pool::<T, _>(|pool| {
            // SAFETY: the router selected `pool` for `Layout::new::<T>()`,
            // which is `alloc_slot`'s precondition.
            let slot = unsafe { pool.alloc_slot::<T>() }?;
            // SAFETY: `slot` was just popped and is owned exclusively here.
            unsafe { occupy_local(slot, value) };
            Ok(Alloc::from_slot(slot))
        })
    }

    /// Fallible [`alloc_with`](Self::alloc_with).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `f` is not called.
    #[inline]
    pub fn try_alloc_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Alloc<'_, T, A>, AllocError> {
        let mut uninit = self.try_alloc_uninit::<T>()?;
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
    pub fn alloc_rc<T>(&self, value: T) -> Rc<T, A> {
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
    pub fn alloc_rc_with<T, F: FnOnce() -> T>(&self, f: F) -> Rc<T, A> {
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
    pub fn try_alloc_rc<T>(&self, value: T) -> Result<Rc<T, A>, AllocError> {
        self.with_pool::<T, _>(|pool| {
            // SAFETY: the router selected `pool` for `Layout::new::<T>()`,
            // which is `alloc_slot`'s precondition.
            let slot = unsafe { pool.alloc_slot::<T>() }?;
            // SAFETY: `slot` was just popped and is owned exclusively here.
            unsafe { pool.core().occupy(slot, value) };
            Ok(Rc::from_slot(slot))
        })
    }

    /// Fallible [`alloc_rc_with`](Self::alloc_rc_with).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `f` is not called.
    #[inline]
    pub fn try_alloc_rc_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Rc<T, A>, AllocError> {
        let mut uninit = self.try_alloc_uninit_rc::<T>()?;
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
    pub fn alloc_rc_pin<T>(&self, value: T) -> Pin<Rc<T, A>> {
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
    pub fn alloc_rc_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> Pin<Rc<T, A>> {
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
    pub fn try_alloc_rc_pin<T>(&self, value: T) -> Result<Pin<Rc<T, A>>, AllocError> {
        let fresh = self.try_alloc_rc(value)?;
        // SAFETY: `fresh` was just constructed here and no alias has escaped.
        Ok(unsafe { Rc::into_pin_fresh(fresh) })
    }

    /// Fallible [`alloc_rc_pin_with`](Self::alloc_rc_pin_with).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available; `f` is not called.
    #[inline]
    pub fn try_alloc_rc_pin_with<T, F: FnOnce() -> T>(&self, f: F) -> Result<Pin<Rc<T, A>>, AllocError> {
        let fresh = self.try_alloc_rc_with(f)?;
        // SAFETY: `fresh` was just constructed here and no alias has escaped.
        Ok(unsafe { Rc::into_pin_fresh(fresh) })
    }

    // ─── uninitialized placement ─────────────────────────────────────────

    /// Reserves a slot for `T` and returns an uninitialized [`Box`], for placing
    /// a value directly into pool memory. Call
    /// [`assume_init`](crate::Box::assume_init) once written.
    ///
    /// # Panics
    /// Panics if allocation fails.
    ///
    /// ```
    /// use plurality::MultiPool;
    ///
    /// let pool = MultiPool::new();
    /// let mut slot = pool.alloc_uninit_box::<u64>();
    /// slot.write(7);
    /// // SAFETY: the value was just written.
    /// let value = unsafe { slot.assume_init() };
    /// assert_eq!(*value, 7);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_uninit_box<T>(&self) -> Box<MaybeUninit<T>, A> {
        match self.try_alloc_uninit_box::<T>() {
            Ok(b) => b,
            Err(err) => allocation_failed(err),
        }
    }

    /// Fallible [`alloc_uninit_box`](Self::alloc_uninit_box).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available.
    #[inline]
    pub fn try_alloc_uninit_box<T>(&self) -> Result<Box<MaybeUninit<T>, A>, AllocError> {
        self.with_pool::<T, _>(|pool| {
            // SAFETY: the router selected `pool` for `Layout::new::<T>()`,
            // which is `alloc_slot`'s precondition.
            let slot = unsafe { pool.alloc_slot::<T>() }?;
            // A `Box` never reads the slot refcount, so (like `Alloc`) only
            // the pool refcount needs bumping here.
            pool.core().bump_pool_ref();
            Ok(Box::from_slot(slot.cast::<SlotCell<MaybeUninit<T>>>()))
        })
    }

    /// Reserves a slot for `T` and returns an uninitialized [`Arc`]. Call
    /// [`assume_init`](crate::Arc::assume_init) once written.
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_arc<T>(&self) -> Arc<MaybeUninit<T>, A>
    where
        T: Send + Sync,
    {
        match self.try_alloc_uninit_arc::<T>() {
            Ok(a) => a,
            Err(err) => allocation_failed(err),
        }
    }

    /// Fallible [`alloc_uninit_arc`](Self::alloc_uninit_arc).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available.
    #[inline]
    pub fn try_alloc_uninit_arc<T>(&self) -> Result<Arc<MaybeUninit<T>, A>, AllocError>
    where
        T: Send + Sync,
    {
        self.with_pool::<T, _>(|pool| {
            // SAFETY: the router selected `pool` for `Layout::new::<T>()`,
            // which is `alloc_slot`'s precondition.
            let slot = unsafe { pool.alloc_slot::<T>() }?;
            // SAFETY: freshly popped; mark occupied without writing a value.
            unsafe { pool.core().mark_occupied(slot) };
            Ok(Arc::from_slot(slot.cast::<SlotCell<MaybeUninit<T>>>()))
        })
    }

    /// Reserves a slot for `T` and returns an uninitialized [`Alloc`].
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[must_use]
    #[inline]
    pub fn alloc_uninit<T>(&self) -> Alloc<'_, MaybeUninit<T>, A> {
        match self.try_alloc_uninit::<T>() {
            Ok(a) => a,
            Err(err) => allocation_failed(err),
        }
    }

    /// Fallible [`alloc_uninit`](Self::alloc_uninit).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available.
    #[inline]
    pub fn try_alloc_uninit<T>(&self) -> Result<Alloc<'_, MaybeUninit<T>, A>, AllocError> {
        self.with_pool::<T, _>(|pool| {
            // SAFETY: the router selected `pool` for `Layout::new::<T>()`,
            // which is `alloc_slot`'s precondition.
            let slot = unsafe { pool.alloc_slot::<T>() }?;
            // An `Alloc` never reads the slot refcount (`push_free`
            // overwrites it on drop), so skip initializing it and
            // `pool_refcount`.
            Ok(Alloc::from_slot(slot.cast::<SlotCell<MaybeUninit<T>>>()))
        })
    }

    /// Reserves a slot for `T` and returns an uninitialized [`Rc`].
    ///
    /// # Panics
    /// Panics if allocation fails.
    #[must_use]
    #[inline]
    pub fn alloc_uninit_rc<T>(&self) -> Rc<MaybeUninit<T>, A> {
        match self.try_alloc_uninit_rc::<T>() {
            Ok(r) => r,
            Err(err) => allocation_failed(err),
        }
    }

    /// Fallible [`alloc_uninit_rc`](Self::alloc_uninit_rc).
    ///
    /// # Errors
    /// Returns [`AllocError`] if no slot is available.
    #[inline]
    pub fn try_alloc_uninit_rc<T>(&self) -> Result<Rc<MaybeUninit<T>, A>, AllocError> {
        self.with_pool::<T, _>(|pool| {
            // SAFETY: the router selected `pool` for `Layout::new::<T>()`,
            // which is `alloc_slot`'s precondition.
            let slot = unsafe { pool.alloc_slot::<T>() }?;
            // SAFETY: freshly popped; mark occupied without writing a value.
            unsafe { pool.core().mark_occupied(slot) };
            Ok(Rc::from_slot(slot.cast::<SlotCell<MaybeUninit<T>>>()))
        })
    }
}
