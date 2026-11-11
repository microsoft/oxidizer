// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc alloc.c, free.c, page.c, page-queue.c and theap.c.
// See LICENSE-MIMALLOC.

//! Page ownership, free lists and thread-local queues.
//!
//! A live block pins its page. Only the owner of the low bit in `remote` can
//! access `local`. Other threads can read immutable page geometry and push a
//! block onto `remote`. Arena bitmaps pin abandoned metadata while readers try
//! to acquire ownership. Destruction requires ownership and no bitmap reader.

use std::alloc::Layout;
use std::cell::{Cell, UnsafeCell};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::backend::abort::require;
use crate::backend::mimalloc::abandoned::{self, Registration};
use crate::backend::mimalloc::classes::{self, BIN_COUNT, DIRECT_COUNT, HUGE_BIN, MEDIUM_MAX, Request, SLICE, SMALL_MAX};
use crate::backend::mimalloc::memory::{self, Memory};
use crate::backend::mimalloc::{atomic, hal, observation, pagemap};

const OWNED: usize = 1;
const INTERIOR: usize = 2; // types.h:MI_PAGE_HAS_INTERIOR_POINTERS.
const OWNER_FLAGS: usize = 3; // types.h:MI_PAGE_FLAG_MASK.
const MAX_EXTEND: usize = 8192; // page.c:MI_MAX_EXTEND_SIZE, bytes touched per extension.
const RETIRE_CYCLES: u8 = 16;
const RETIRE_MAX_PAGES: usize = 3;
const MAX_CANDIDATES: usize = 4;
const SMALL_RETAIN: usize = 2;

#[derive(Clone, Copy)]
struct Queue {
    first: usize,
    last: usize,
    count: usize,
}

impl Queue {
    const EMPTY: Self = Self {
        first: 0,
        last: 0,
        count: 0,
    };
}

struct PageState {
    free: usize,
    local_free: usize,
    free_is_zero: bool,
    capacity: usize,
    used: usize,
    retire: u8,
    previous: usize,
    next: usize,
    mapped: bool,
}

#[repr(align(64))]
struct Page {
    self_pointer: AtomicUsize,
    owner: AtomicUsize,
    local: UnsafeCell<PageState>,
    block_size: usize,
    bin: usize,
    start: usize,
    reserved: usize,
    original_owner: AtomicUsize,
    remote: AtomicUsize,
    abandoned: Registration,
    memory: Memory,
}

impl Page {
    /// # Safety
    /// The caller owns the page's remote-list ownership bit and performs the
    /// corresponding queue transition. Concurrent interior-flag updates are allowed.
    unsafe fn set_owner(&self, owner: usize) {
        let updated = atomic::try_update(&self.owner, Ordering::Release, Ordering::Relaxed, |previous| {
            Some(owner | (previous & OWNER_FLAGS))
        });
        require(updated.is_ok());
    }
}

pub(super) const PAGE_META_SIZE: usize = std::mem::size_of::<Page>();
pub(super) const PAGE_SELF_OFFSET: usize = std::mem::offset_of!(Page, self_pointer);
const _: () = assert!(
    PAGE_META_SIZE <= 4096,
    "page metadata must fit the 4-KiB allowance used to derive the size classes"
);

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

struct Heap {
    id: usize,
    sequence: usize,
    queues: [Queue; BIN_COUNT],
    direct: [usize; DIRECT_COUNT],
    generic_count: usize,
    generic_collect_count: usize,
    retired_min: usize,
    retired_max: usize,
    in_threadpool: bool,
    reclaim_on_free: bool,
}

impl Heap {
    const fn new() -> Self {
        Self {
            id: 0,
            sequence: 0,
            queues: [Queue::EMPTY; BIN_COUNT],
            direct: [0; DIRECT_COUNT],
            generic_count: 0,
            generic_collect_count: 0,
            retired_min: HUGE_BIN,
            retired_max: 0,
            in_threadpool: false,
            reclaim_on_free: true,
        }
    }

    fn initialize(&mut self) {
        if self.id == 0 {
            self.id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            require(self.id != 0 && self.id <= usize::MAX / 16);
            self.id *= 16; // Reserve the low bits for page flags and abandoned states.
            self.sequence = memory::sequence();
            self.in_threadpool = hal::is_in_threadpool();
        }
    }
}

struct ThreadHeap {
    borrowed: Cell<bool>,
    heap: UnsafeCell<Heap>,
}

struct ThreadStorage {
    thread: ThreadHeap,
    current: Cell<*const ThreadHeap>,
}

struct HeapLifetime;

const _: () = assert!(
    !std::mem::needs_drop::<ThreadStorage>(),
    "heap storage must remain accessible to its separately registered cleanup"
);

std::thread_local! {
    // Store the heap and cached pointer in one non-dropping TLS value so both
    // reads share a thread-local address.
    static STORAGE: ThreadStorage = const {
        ThreadStorage {
            thread: ThreadHeap { borrowed: Cell::new(false), heap: UnsafeCell::new(Heap::new()) },
            current: Cell::new(std::ptr::null()),
        }
    };
    static HEAP_LIFETIME: HeapLifetime = const { HeapLifetime };
}

impl Drop for HeapLifetime {
    fn drop(&mut self) {
        STORAGE.with(|storage| {
            storage.current.set(std::ptr::null());
            // SAFETY: This cleanup runs once on the owner thread. The cache is
            // clear, so reentrant operations cannot borrow this heap. Remote
            // frees only access page atomics and their own blocks.
            let heap = unsafe { &mut *storage.thread.heap.get() };
            // SAFETY: Cleanup uniquely owns the heap until finish returns.
            unsafe { heap.finish() };
            observation::retire();
        });
    }
}

impl ThreadHeap {
    #[inline]
    fn allocate<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        require(!self.borrowed.get());
        // SAFETY: The guard excludes an active exclusive borrow. The direct
        // path only reads Heap and changes the owned page through UnsafeCell.
        // It cannot reenter the allocator. This shared borrow ends before with.
        let heap = unsafe { &*self.heap.get() };
        if let Some(pointer) = heap.try_allocate::<ZEROED>(layout) {
            return pointer;
        }
        self.allocate_slow::<ZEROED>(layout)
    }

    // Keep exclusive-borrow cleanup out of the leaf path so it does not reserve
    // a register just to restore the flag after a generic allocation call.
    #[inline(never)]
    fn allocate_slow<const ZEROED: bool>(&self, layout: Layout) -> *mut u8 {
        self.with(|heap| heap.allocate_generic::<ZEROED>(layout))
    }

    /// # Safety
    /// The caller retires this backend's live allocation exactly once.
    #[inline]
    unsafe fn deallocate(&self, pointer: *mut u8) {
        require(!self.borrowed.get());
        // SAFETY: The guard excludes an active exclusive heap borrow, and the
        // copied identity is immutable throughout this thread's heap lifetime.
        let id = unsafe { (*self.heap.get()).id };
        // SAFETY: The live block pins its registered page.
        let address = unsafe { pagemap::lookup(pointer.addr()) };
        // SAFETY: The live allocation pins initialized page geometry.
        let data = unsafe { page(address) };
        let owner = data.owner.load(Ordering::Acquire);
        if owner == id {
            // SAFETY: The identity grants exclusive page-local access. This
            // leaf operation cannot reenter or borrow the Heap.
            if unsafe { free_local(address, pointer.addr()) } {
                // SAFETY: The page is still queued and has no live blocks.
                unsafe { self.retire(address, data.bin) };
            }
        } else {
            // SAFETY: The allocation is live and its owner was acquired above.
            unsafe { self.deallocate_slow(pointer, address, owner) };
        }
    }

    // Queue transitions retain the exclusive borrow; ordinary frees need none.
    #[inline(never)]
    unsafe fn retire(&self, address: usize, bin: usize) {
        self.with(|heap| {
            // SAFETY: The empty page remains in this heap's queue, and with
            // grants the exclusive borrow needed to update that queue.
            unsafe { heap.retire(bin, address) };
        });
    }

    // The remote and aligned paths must not add register saves to local frees.
    #[inline(never)]
    unsafe fn deallocate_slow(&self, pointer: *mut u8, address: usize, owner: usize) {
        self.with(|heap| {
            // SAFETY: The allocation is still live, and with excludes
            // reentrancy for the generic queue and remote-free transitions.
            unsafe { heap.deallocate_generic(pointer, address, owner) };
        });
    }

    #[inline]
    fn with<R>(&self, action: impl FnOnce(&mut Heap) -> R) -> R {
        require(!self.borrowed.get());
        self.borrowed.set(true);
        // SAFETY: The thread-local borrow flag excludes reentrancy. No reference
        // to the heap escapes this synchronous callback.
        let heap = unsafe { &mut *self.heap.get() };
        let result = action(heap);
        self.borrowed.set(false);
        result
    }
}

fn with_heap<R>(action: impl FnOnce(&mut Heap) -> R) -> R {
    STORAGE.with(|storage| {
        if storage.current.get().is_null() {
            return with_initialized_heap(action);
        }
        storage.thread.with(action)
    })
}

// Keep registration and teardown recovery out of the steady-state allocation
// body. Otherwise downstream Windows codegen spills registers and refuses to
// inline the direct allocation path into callers with constant layouts.
#[cold]
#[inline(never)]
fn with_initialized_heap<R>(action: impl FnOnce(&mut Heap) -> R) -> R {
    if HEAP_LIFETIME.try_with(|_| ()).is_err() {
        return with_temporary_heap(action);
    }
    STORAGE.with(|storage| {
        let thread = &storage.thread;
        thread.with(|heap| {
            heap.initialize();
            observation::register(heap.id);
            storage.current.set(std::ptr::from_ref(thread));
            action(heap)
        })
    })
}

#[cold]
#[inline(never)]
fn allocate_uninitialized<const ZEROED: bool>(layout: Layout) -> *mut u8 {
    if let Ok(pointer) = HEAP_LIFETIME.try_with(|_| {
        // Select before heap initialization: native null TLS slots skip the
        // naturally-aligned shortcut on the first aligned request.
        let Some(request) = Request::for_new_thread(layout) else {
            return std::ptr::null_mut();
        };
        STORAGE.with(|storage| {
            let thread = &storage.thread;
            thread.with(|heap| {
                heap.initialize();
                observation::register(heap.id);
                storage.current.set(std::ptr::from_ref(thread));
                heap.allocate_request::<ZEROED>(request)
            })
        })
    }) {
        pointer
    } else {
        // Native teardown publishes the non-null empty theap. Preserve
        // that selection and validate before creating a temporary heap.
        let Some(request) = Request::new(layout) else {
            return std::ptr::null_mut();
        };
        with_temporary_heap(|heap| heap.allocate_request::<ZEROED>(request))
    }
}

// Keep late-teardown storage off the hot path. Optimized Windows assembly
// otherwise reserves a 2872-byte frame on every allocation and deallocation.
#[cold]
#[inline(never)]
fn with_temporary_heap<R>(action: impl FnOnce(&mut Heap) -> R) -> R {
    let mut heap = Heap::new();
    heap.initialize();
    heap.reclaim_on_free = false; // Native free.c forbids reclamation after TLS teardown.
    let result = action(&mut heap);
    // SAFETY: The temporary heap is exclusively owned by this callback.
    unsafe { heap.finish() };
    result
}

pub(super) fn observe_current(result: &mut seismograph_rallocator::mimalloc::Observation) -> bool {
    STORAGE
        .try_with(|storage| {
            if storage.current.get().is_null() || storage.thread.borrowed.get() {
                return false;
            }
            storage.thread.with(|heap| heap.observe(result));
            true
        })
        .unwrap_or(false)
}

impl Heap {
    fn observe(&self, result: &mut seismograph_rallocator::mimalloc::Observation) {
        self.observe_with_budget(result, observation::WALK_BUDGET);
    }

    fn observe_with_budget(&self, result: &mut seismograph_rallocator::mimalloc::Observation, mut budget: usize) {
        const _: () = assert!(BIN_COUNT == seismograph_rallocator::mimalloc::CLASS_COUNT);
        for (bin, class) in result.classes.iter_mut().enumerate() {
            if (1..=60).contains(&bin) {
                class.block_bytes = classes::bin_size(bin) as u64;
                class.page_bytes = classes::page_size(classes::bin_size(bin)) as u64;
            }
            class.queued_pages = self.queues[bin].count as u64;
            let mut address = self.queues[bin].first;
            while address != 0 {
                if budget == 0 {
                    result.pages_complete = false;
                    break;
                }
                budget -= 1;
                // SAFETY: observe_current holds this thread's exclusive heap
                // borrow. Every queued page remains owned until the copy ends.
                let state = unsafe { local(address) };
                // SAFETY: Queue membership pins immutable page geometry.
                let data = unsafe { page(address) };
                class.observed_pages += 1;
                class.reserved_blocks += data.reserved as u64;
                class.capacity_blocks += state.capacity as u64;
                class.used_blocks += state.used as u64;
                class.retired_pages += u64::from(state.retire != 0);
                // SAFETY: Both local lists belong exclusively to this heap.
                let (free, complete) = unsafe { observe_list(state.free, &mut budget) };
                class.free_blocks += free;
                result.free_lists_complete &= complete;
                // SAFETY: The owner-local deferred list has no concurrent writer.
                let (free, complete) = unsafe { observe_list(state.local_free, &mut budget) };
                class.local_free_blocks += free;
                result.free_lists_complete &= complete;
                address = state.next;
            }
        }
    }
}

/// # Safety
/// The caller owns a local free list and excludes every mutation until return.
unsafe fn observe_list(mut head: usize, budget: &mut usize) -> (u64, bool) {
    let mut count = 0;
    while head != 0 {
        if *budget == 0 {
            return (count, false);
        }
        *budget -= 1;
        count += 1;
        // SAFETY: The owner excludes reuse and mutation of this initialized link.
        head = unsafe { next(head) };
    }
    (count, true)
}

/// # Safety
/// The metadata address is live and initialized. The caller pins its page
/// through a live block, queue membership, or an abandoned bitmap reader for the
/// entire returned borrow.
unsafe fn page<'a>(address: usize) -> &'a Page {
    // SAFETY: The caller guarantees initialized, aligned, live page storage.
    unsafe { &*std::ptr::with_exposed_provenance::<Page>(address) }
}

/// # Safety
/// The caller owns the page's ownership bit, and no other local borrow exists.
unsafe fn local<'a>(address: usize) -> &'a mut PageState {
    // SAFETY: The ownership bit excludes other accesses to local state.
    let data = unsafe { page(address) };
    // SAFETY: The caller guarantees unique access to this page's local state.
    unsafe { &mut *data.local.get() }
}

/// # Safety
/// The block is free, at least word-sized and word-aligned, and exclusively
/// owned by the caller or published in a list acquired by the caller.
unsafe fn next(block: usize) -> usize {
    // SAFETY: The caller guarantees an initialized free-list link.
    unsafe { std::ptr::with_exposed_provenance::<usize>(block).read() }
}

/// # Safety
/// The block is free, at least word-sized and word-aligned, and exclusively owned.
unsafe fn set_next(block: usize, value: usize) {
    // SAFETY: The caller owns this free block's link word.
    unsafe { std::ptr::with_exposed_provenance_mut::<usize>(block).write(value) };
}

/// # Safety
/// The caller owns the page, its queues no longer reference it, all live blocks
/// are gone, and the page has been removed from the abandoned list.
unsafe fn destroy(address: usize) {
    // SAFETY: Exclusive page ownership grants access to its immutable geometry.
    let data = unsafe { page(address) };
    let mapped_size = if data.bin == HUGE_BIN {
        data.memory.len().min(2 * SLICE)
    } else {
        data.memory.len()
    };
    // SAFETY: All blocks, remote messages and abandoned readers are retired.
    unsafe { pagemap::unregister(data.memory.as_ptr().addr(), mapped_size) };
    // SAFETY: Move the only owning field out before releasing its backing.
    // For small pages the Page itself resides in that backing. No Page field
    // is accessed or dropped after this move.
    let memory = unsafe { std::ptr::from_ref(&data.memory).read() };
    drop(memory);
}

fn fresh(request: Request, owner: usize) -> Option<usize> {
    let block_size = request.block_size()?;
    let singleton = request.bin() == HUGE_BIN;
    let size = if singleton { block_size } else { classes::page_size(block_size) };
    let memory = if singleton {
        Memory::allocate_page(size, request.alignment.max(SLICE), size.div_ceil(SLICE).min(2))?
    } else {
        Memory::allocate(size, SLICE)?
    };
    let initially_zero = memory.is_zero();
    let abandoned = Registration::new(&memory, request.bin())?;
    let base = memory.as_ptr().expose_provenance();
    let inline_metadata = !singleton && block_size <= SMALL_MAX;
    // arena.c:mi_page_block_start. Small-page metadata lives before the blocks.
    let offset = if inline_metadata {
        let info = PAGE_META_SIZE.next_multiple_of(16);
        if block_size.is_power_of_two() && block_size <= 4096 {
            info.next_multiple_of(block_size) + if block_size < 64 { 3 * block_size } else { 0 }
        } else if block_size.is_multiple_of(4096) {
            info.next_multiple_of(4096)
        } else {
            info
        }
    } else {
        0
    };
    let reserved = if singleton { 1 } else { (size - offset) / block_size };
    let address = if inline_metadata {
        base
    } else {
        memory.metadata_ptr().expose_provenance()
    };
    let value = Page {
        self_pointer: AtomicUsize::new(address),
        owner: AtomicUsize::new(owner),
        local: UnsafeCell::new(PageState {
            free: 0,
            local_free: 0,
            free_is_zero: initially_zero,
            capacity: 0,
            used: 0,
            retire: 0,
            previous: 0,
            next: 0,
            mapped: false,
        }),
        block_size,
        bin: request.bin(),
        start: base + offset,
        reserved,
        original_owner: AtomicUsize::new(owner),
        remote: AtomicUsize::new(OWNED),
        abandoned,
        memory,
    };
    // SAFETY: Backing creation grants aligned, exclusive, committed metadata.
    unsafe { std::ptr::with_exposed_provenance_mut::<Page>(address).write(value) };
    // SAFETY: The new page is exclusively owned and not yet published.
    let data = unsafe { page(address) };
    let mapped_size = if singleton {
        data.memory.len().min(2 * SLICE)
    } else {
        data.memory.len()
    };
    // SAFETY: All relevant aligned metadata slots are committed by Memory.
    unsafe { pagemap::register(base, mapped_size, address) };
    // SAFETY: The fresh page's ownership bit belongs to this call.
    unsafe { extend(address) };
    Some(address)
}

/// # Safety
/// The caller owns the page and both local free lists are empty.
unsafe fn extend(address: usize) {
    // SAFETY: The caller owns page-local state.
    let data = unsafe { page(address) };
    // SAFETY: No other owner-local borrow exists.
    let state = unsafe { local(address) };
    let count = (data.reserved - state.capacity).min((MAX_EXTEND / data.block_size).max(1));
    if count == 0 {
        return;
    }
    let first = data.start + state.capacity * data.block_size;
    for index in 0..count {
        let block = first + index * data.block_size;
        let following = if index + 1 == count { state.free } else { block + data.block_size };
        // SAFETY: Each block is within exclusive committed backing and not live.
        unsafe { set_next(block, following) };
    }
    state.free = first;
    state.capacity += count;
}

/// # Safety
/// The caller owns page-local state and the detached remote free list.
unsafe fn collect_list(address: usize, head: usize) {
    if head == 0 {
        return;
    }
    // SAFETY: The caller holds page ownership.
    let state = unsafe { local(address) };
    let mut tail = head;
    let mut count = 1;
    loop {
        // SAFETY: The list was acquired atomically and has not been republished.
        let following = unsafe { next(tail) };
        if following == 0 {
            break;
        }
        count += 1;
        require(count <= state.used);
        tail = following;
    }
    require(count <= state.used);
    // SAFETY: The detached list's final block is exclusively owned.
    unsafe { set_next(tail, state.local_free) };
    state.local_free = head;
    state.used -= count;
}

/// # Safety
/// The caller owns page-local state.
unsafe fn collect(address: usize) {
    // SAFETY: The page remains live under the caller's ownership.
    let data = unsafe { page(address) };
    let mut previous = data.remote.load(Ordering::Relaxed);
    while previous & !OWNED != 0 {
        match data
            .remote
            .compare_exchange_weak(previous, previous & OWNED, Ordering::AcqRel, Ordering::Relaxed)
        {
            Ok(_) => {
                // SAFETY: Acquire detached the list, retaining page ownership.
                unsafe { collect_list(address, previous & !OWNED) };
                break;
            }
            Err(current) => previous = current,
        }
    }
    // SAFETY: collect_list's local borrow ended.
    let state = unsafe { local(address) };
    if state.free == 0 && state.local_free != 0 {
        state.free = state.local_free;
        state.local_free = 0;
        state.free_is_zero = false;
    }
}

/// # Safety
/// The caller owns the page and has no outstanding local-state borrow.
unsafe fn collect_force(address: usize) {
    // SAFETY: The caller owns all local lists.
    unsafe { collect(address) };
    // SAFETY: The collection's mutable borrow ended.
    let state = unsafe { local(address) };
    if state.local_free == 0 {
        return;
    }
    let mut tail = state.local_free;
    loop {
        // SAFETY: The local list is owned, initialized, and not concurrently accessed.
        let following = unsafe { next(tail) };
        if following == 0 {
            break;
        }
        tail = following;
    }
    // SAFETY: This final free block belongs exclusively to the caller.
    unsafe { set_next(tail, state.free) };
    state.free = state.local_free;
    state.local_free = 0;
    state.free_is_zero = false;
}

/// # Safety
/// The caller acquired this abandoned page by publishing `head` onto its
/// remote list. The head remains in that list while this operation runs.
unsafe fn collect_partly(address: usize, head: usize) -> usize {
    // SAFETY: The published head's link is initialized and the caller owns the page.
    let following = unsafe { next(head) };
    if following != 0 {
        // No other publisher reads or writes an existing head's next link.
        // SAFETY: Only this page owner accesses this published link.
        unsafe { set_next(head, 0) };
        // SAFETY: The detached suffix belongs exclusively to the page owner.
        unsafe { collect_list(address, following) };
        // SAFETY: The collection's local borrow ended.
        let state = unsafe { local(address) };
        if state.free == 0 {
            state.free = state.local_free;
            state.local_free = 0;
            state.free_is_zero = false;
        }
    }
    // SAFETY: The caller still owns page-local state.
    if unsafe { local(address) }.used == 1 {
        // The uncollected head is the last used block. Collect it so an
        // all-free page cannot remain stranded waiting for a future free.
        // SAFETY: The caller owns the entire remote list.
        unsafe { collect(address) };
        0
    } else {
        head
    }
}

/// # Safety
/// The caller owns a page with a nonempty free list. Its block covers the
/// unaligned request, as established by the direct or generic path.
#[inline]
unsafe fn pop<const ZEROED: bool>(address: usize) -> *mut u8 {
    // SAFETY: The caller owns the page and its local state.
    let data = unsafe { page(address) };
    // SAFETY: No other local borrow exists.
    let state = unsafe { local(address) };
    let block = state.free;
    require(block != 0);
    state.used += 1;
    // SAFETY: The owned free block contains an initialized list link.
    state.free = unsafe { next(block) };
    let pointer = std::ptr::with_exposed_provenance_mut::<u8>(block);
    if ZEROED {
        if state.free_is_zero {
            // SAFETY: The remaining bytes are already zero. Only the free-list
            // word changed since the OS supplied this block.
            unsafe { set_next(block, 0) };
        } else {
            let block = std::ptr::with_exposed_provenance_mut::<u8>(block);
            // SAFETY: The caller owns this entire block, including alignment padding.
            unsafe { block.write_bytes(0, data.block_size) };
        }
    }
    pointer
}

/// # Safety
/// The caller owns page-local state and retires this live base block once.
/// The result indicates that the owning heap must retire the empty page.
#[inline]
unsafe fn free_local(address: usize, block: usize) -> bool {
    // SAFETY: Page ownership grants exclusive access to both local lists.
    let state = unsafe { local(address) };
    // SAFETY: The live block becomes the uniquely owned local free-list head.
    unsafe { set_next(block, state.local_free) };
    state.local_free = block;
    state.used -= 1;
    state.used == 0 && state.retire == 0
}

/// # Safety
/// The caller owns the abandoned page.
unsafe fn unmap_abandoned(address: usize) {
    // SAFETY: Page ownership keeps the metadata live.
    let data = unsafe { page(address) };
    // SAFETY: The caller holds the page's ownership bit.
    if !unsafe { local(address) }.mapped {
        return;
    }
    data.abandoned.remove();
    // SAFETY: The caller still owns page-local state.
    unsafe { local(address) }.mapped = false;
}

/// # Safety
/// The caller owns the abandoned page and has removed it from owner queues.
unsafe fn map_abandoned(address: usize) {
    // SAFETY: Page ownership keeps metadata live.
    let data = unsafe { page(address) };
    // SAFETY: Page-local state is exclusively owned.
    let state = unsafe { local(address) };
    if state.mapped || data.bin == HUGE_BIN {
        return;
    }
    // SAFETY: Ownership is retained and the page is initialized and registered.
    state.mapped = unsafe { data.abandoned.publish() };
}

/// # Safety
/// The caller owns the abandoned page, with no remaining mutable local borrow.
unsafe fn release_abandoned(address: usize, mut expected_head: usize) {
    loop {
        // SAFETY: The caller has sole page ownership.
        let state = unsafe { local(address) };
        if state.used == 0 {
            // SAFETY: Ownership is retained while unlinking and retiring the page.
            unsafe { unmap_abandoned(address) };
            // SAFETY: All blocks and remote messages are retired; no list references remain.
            unsafe { destroy(address) };
            return;
        }
        // free.c remaps formerly full pages below the seven-eighths threshold.
        // SAFETY: Page ownership permits reading immutable geometry.
        let data = unsafe { page(address) };
        if state.used < data.reserved - data.reserved / 8 {
            // SAFETY: This page is abandoned and exclusively owned.
            unsafe { map_abandoned(address) };
        }
        if data
            .remote
            .compare_exchange(expected_head | OWNED, expected_head, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return; // Do not access the page after relinquishing ownership.
        }
        // New remote frees arrived after the captured head. Collect them before
        // retrying so a concurrent final free cannot strand an empty page.
        // SAFETY: The failed CAS retained exclusive page ownership.
        unsafe { collect(address) };
        expected_head = 0;
    }
}

fn reclaim(bin: usize, owner: usize, sequence: usize) -> Option<usize> {
    let claim = |address| {
        // SAFETY: The abandoned bitmap reader pins this registered page.
        let data = unsafe { page(address) };
        data.remote.fetch_or(OWNED, Ordering::AcqRel) & OWNED == 0
    };
    // SAFETY: The callback retains no reference when acquisition fails. On
    // success it replaces the bitmap pin with exclusive page ownership.
    let address = unsafe { abandoned::reclaim(bin, sequence, claim) }?;
    // SAFETY: The reader transferred exclusive page ownership.
    unsafe { local(address) }.mapped = false;
    // SAFETY: Ownership pins all page fields until the page is abandoned again.
    let data = unsafe { page(address) };
    // SAFETY: Reclaiming the bitmap entry transferred exclusive page ownership.
    unsafe { data.set_owner(owner) };
    data.original_owner.store(owner, Ordering::Relaxed);
    // SAFETY: The reclaimed page remains exclusively owned.
    unsafe { collect(address) };
    Some(address)
}

impl Heap {
    /// # Safety
    /// This heap owns the page and it is not in any local queue.
    unsafe fn push(&mut self, bin: usize, address: usize, back: bool) {
        let queue = &mut self.queues[bin];
        // SAFETY: This heap owns the page.
        let state = unsafe { local(address) };
        if back {
            state.previous = queue.last;
            state.next = 0;
            if queue.last != 0 {
                // SAFETY: The same heap owns the former tail.
                unsafe { local(queue.last) }.next = address;
            } else {
                queue.first = address;
            }
            queue.last = address;
        } else {
            state.previous = 0;
            state.next = queue.first;
            if queue.first != 0 {
                // SAFETY: The same heap owns the former head.
                unsafe { local(queue.first) }.previous = address;
            } else {
                queue.last = address;
            }
            queue.first = address;
        }
        queue.count += 1;
        self.update_direct(bin);
    }

    /// # Safety
    /// This heap owns the page, which is currently in this bin's local queue.
    unsafe fn remove(&mut self, bin: usize, address: usize) {
        // SAFETY: This heap owns the page.
        let state = unsafe { local(address) };
        let previous = state.previous;
        let following = state.next;
        state.previous = 0;
        state.next = 0;
        if previous == 0 {
            self.queues[bin].first = following;
        } else {
            // SAFETY: This heap owns the preceding page in its queue.
            unsafe { local(previous) }.next = following;
        }
        if following == 0 {
            self.queues[bin].last = previous;
        } else {
            // SAFETY: This heap owns the following page in its queue.
            unsafe { local(following) }.previous = previous;
        }
        self.queues[bin].count -= 1;
        self.update_direct(bin);
    }

    fn update_direct(&mut self, bin: usize) {
        if bin == HUGE_BIN {
            return;
        }
        let size = classes::bin_size(bin);
        if size <= 1024 {
            let previous = if bin > 2 && bin <= 8 { bin - 2 } else { bin - 1 };
            let minimum = if bin == 1 { 0 } else { classes::bin_size(previous) / 8 + 1 };
            let maximum = size / 8;
            self.direct[minimum..=maximum].fill(self.queues[bin].first);
        }
    }

    /// # Safety
    /// This heap owns the page and it is in its bin's local queue.
    unsafe fn abandon(&mut self, bin: usize, address: usize) {
        // SAFETY: This heap owns the queued page.
        unsafe { collect(address) };
        // SAFETY: Queue membership belongs exclusively to this heap.
        unsafe { self.remove(bin, address) };
        // SAFETY: This heap retains ownership until release_abandoned finishes.
        let data = unsafe { page(address) };
        data.owner.fetch_and(OWNER_FLAGS, Ordering::Release);
        // SAFETY: The local queue no longer references this page.
        let state = unsafe { local(address) };
        if state.used == 0 {
            // SAFETY: Collection retired all blocks and queue removal retired
            // the last external page reference.
            unsafe { destroy(address) };
            return;
        }
        if state.used < data.reserved {
            // SAFETY: The page is abandoned but still exclusively owned.
            unsafe { map_abandoned(address) };
        }
        // SAFETY: Abandonment retains ownership until the atomic release.
        unsafe { release_abandoned(address, 0) };
    }

    /// # Safety
    /// This heap owns the page and it is in its bin's local queue.
    unsafe fn retire(&mut self, bin: usize, address: usize) {
        // SAFETY: The page's local state belongs to this heap.
        let state = unsafe { local(address) };
        if state.retire != 0 {
            return;
        }
        // SAFETY: The queued page remains live.
        let data = unsafe { page(address) };
        data.owner.fetch_and(!INTERIOR, Ordering::Relaxed);
        if bin != HUGE_BIN && self.queues[bin].count <= RETIRE_MAX_PAGES && (self.queues[bin].count == 1 || data.block_size < 1024) {
            state.retire = if data.block_size <= SMALL_MAX {
                RETIRE_CYCLES
            } else {
                RETIRE_CYCLES / 4
            };
            self.retired_min = self.retired_min.min(bin);
            self.retired_max = self.retired_max.max(bin);
        } else {
            // SAFETY: This heap owns all local queue links.
            unsafe { self.remove(bin, address) };
            // SAFETY: used is zero, so no remote publisher or block can remain.
            unsafe { destroy(address) };
        }
    }

    /// # Safety
    /// This heap is used exclusively by its owning thread.
    unsafe fn collect_retired(&mut self, force: bool) {
        let mut minimum = HUGE_BIN;
        let mut maximum = 0;
        for bin in self.retired_min..=self.retired_max {
            let mut address = self.queues[bin].first;
            for _ in 0..RETIRE_MAX_PAGES {
                if address == 0 {
                    break;
                }
                // SAFETY: All queued pages belong exclusively to this heap.
                let state = unsafe { local(address) };
                if state.retire == 0 {
                    break;
                }
                let following = state.next;
                if state.used == 0 {
                    state.retire -= 1;
                    if force || state.retire == 0 {
                        // SAFETY: The page belongs to this queue and has no live blocks.
                        unsafe { self.remove(bin, address) };
                        // SAFETY: Queue removal retired the last reference.
                        unsafe { destroy(address) };
                    } else {
                        minimum = minimum.min(bin);
                        maximum = maximum.max(bin);
                    }
                } else {
                    state.retire = 0;
                }
                address = following;
            }
        }
        self.retired_min = minimum;
        self.retired_max = maximum;
    }

    /// # Safety
    /// The heap is exclusively borrowed by its owning thread.
    unsafe fn collect_all(&mut self, force: bool) {
        // SAFETY: This heap owns every retired page.
        unsafe { self.collect_retired(force) };
        for bin in 0..BIN_COUNT {
            let mut address = self.queues[bin].first;
            while address != 0 {
                // SAFETY: The current queue pins this owned page.
                let following = unsafe { local(address) }.next;
                if force {
                    // SAFETY: This heap owns both local free lists.
                    unsafe { collect_force(address) };
                } else {
                    // SAFETY: This heap owns local state and can acquire remote frees.
                    unsafe { collect(address) };
                }
                // SAFETY: Collection retained page ownership.
                let state = unsafe { local(address) };
                if state.used == 0 && (force || state.retire == 0) {
                    // SAFETY: This page is in the heap's bin queue.
                    unsafe { self.remove(bin, address) };
                    // SAFETY: No allocation, remote message, or queue reference remains.
                    unsafe { destroy(address) };
                }
                address = following;
            }
        }
        Memory::collect(force);
    }

    /// # Safety
    /// This heap is used exclusively by its owning thread.
    unsafe fn find(&mut self, request: Request) -> Option<usize> {
        let bin = request.bin();
        if bin == HUGE_BIN {
            let address = fresh(request, self.id)?;
            // SAFETY: A fresh page is owned by this heap and not queued.
            unsafe { self.push(bin, address, false) };
            return Some(address);
        }
        let first = self.queues[bin].first;
        if first != 0 {
            // SAFETY: The first queued page belongs to this heap.
            let state = unsafe { local(first) };
            if state.free == 0 && state.local_free != 0 {
                state.free = state.local_free;
                state.local_free = 0;
                state.free_is_zero = false;
            }
            if state.free != 0 {
                state.retire = 0;
                return Some(first);
            }
        }
        let mut address = first;
        let last = self.queues[bin].last;
        let mut retained = if classes::bin_size(bin) <= SMALL_MAX {
            if self.in_threadpool { SMALL_RETAIN / 4 } else { SMALL_RETAIN }
        } else {
            0
        };
        let mut candidate = 0;
        let mut remaining: usize = 0;
        while address != 0 {
            remaining = remaining.saturating_sub(1);
            // SAFETY: This heap owns each visited page.
            if unsafe { local(address) }.free == 0 {
                // SAFETY: The page remains in this heap's queue.
                unsafe { collect(address) };
            }
            let (free, capacity, used, following) = {
                // SAFETY: Collection retained exclusive local access.
                let state = unsafe { local(address) };
                (state.free, state.capacity, state.used, state.next)
            };
            // SAFETY: Queue membership pins immutable geometry.
            let data = unsafe { page(address) };
            if free == 0 && capacity == data.reserved {
                if retained == 0 {
                    // SAFETY: The current page belongs to this heap's queue.
                    unsafe { self.abandon(bin, address) };
                } else {
                    retained -= 1;
                    if address != last && address != self.queues[bin].last {
                        // SAFETY: This heap owns the queued page.
                        unsafe { self.remove(bin, address) };
                        // SAFETY: The removed page is owned and not queued.
                        unsafe { self.push(bin, address, true) };
                    }
                }
            } else {
                if candidate == 0 {
                    candidate = address;
                    remaining = MAX_CANDIDATES;
                } else {
                    // SAFETY: The candidate remains queued and owned.
                    let old_used = unsafe { local(candidate) }.used;
                    // An empty initialized page has collected free blocks and
                    // stops the search immediately. A continuing candidate is
                    // therefore nonempty in this non-hardened configuration.
                    require(old_used != 0);
                    if used >= old_used && used < data.reserved - data.reserved / 8 {
                        candidate = address;
                    }
                }
                if free != 0 || remaining == 0 {
                    break;
                }
            }
            address = if address == last { 0 } else { following };
        }
        if candidate != 0 {
            // SAFETY: Candidate is owned and currently queued.
            unsafe { self.remove(bin, candidate) };
            // SAFETY: Candidate is no longer queued and ownership is retained.
            unsafe { self.push(bin, candidate, false) };
            // SAFETY: The candidate's local state belongs to this heap.
            let state = unsafe { local(candidate) };
            state.retire = 0;
            if state.free == 0 {
                // SAFETY: Collection found no free list, but uninitialized capacity remains.
                unsafe { extend(candidate) };
            }
            return Some(candidate);
        }
        // SAFETY: This heap exclusively owns the retired-page queues.
        unsafe { self.collect_retired(false) };
        let reclaimed = reclaim(bin, self.id, self.sequence);
        let address = reclaimed.or_else(|| fresh(request, self.id))?;
        // SAFETY: A fresh or reclaimed page belongs exclusively to this heap.
        unsafe { self.push(bin, address, reclaimed.is_some()) };
        // SAFETY: The page remains owned after insertion.
        if unsafe { local(address) }.free == 0 {
            // SAFETY: A reclaimed page with no immediately free block is expandable.
            unsafe { extend(address) };
        }
        Some(address)
    }

    #[cfg(test)]
    #[inline]
    fn allocate<const ZEROED: bool>(&mut self, layout: Layout) -> *mut u8 {
        self.try_allocate::<ZEROED>(layout)
            .unwrap_or_else(|| self.allocate_generic::<ZEROED>(layout))
    }

    #[inline]
    fn try_allocate<const ZEROED: bool>(&self, layout: Layout) -> Option<*mut u8> {
        // alloc-aligned.c tries the direct table before calculating size classes
        // or overallocating. A misaligned head must take the generic path.
        if layout.size() <= 1024 && layout.align() <= layout.size() {
            let address = self.direct[layout.size().div_ceil(8)];
            if address != 0 {
                // SAFETY: Every direct entry is owned and queued by this heap.
                let free = unsafe { local(address) }.free;
                // Layout guarantees a power-of-two alignment. A general
                // remainder check emits a hardware divide for dynamic layouts.
                if free != 0 && free & (layout.align() - 1) == 0 {
                    // SAFETY: The direct class covers the layout without an alignment adjustment.
                    return Some(unsafe { pop::<ZEROED>(address) });
                }
            }
        }
        None
    }

    // Like mimalloc's generic allocation path, keep queue search and backing
    // acquisition out of the direct-table path. Otherwise Windows codegen
    // spills eight preserved registers before checking an available small block.
    #[inline(never)]
    fn allocate_generic<const ZEROED: bool>(&mut self, layout: Layout) -> *mut u8 {
        let Some(request) = Request::new(layout) else {
            return std::ptr::null_mut();
        };
        self.allocate_request::<ZEROED>(request)
    }

    fn allocate_request<const ZEROED: bool>(&mut self, request: Request) -> *mut u8 {
        let direct_index = request.size.div_ceil(8);
        let mut address = if request.alignment <= SLICE && direct_index < DIRECT_COUNT {
            self.direct[direct_index]
        } else {
            0
        };
        if address != 0 {
            // SAFETY: A direct entry references the first page in this heap's queue.
            if unsafe { local(address) }.free == 0 {
                address = 0;
            }
        }
        if address == 0 {
            self.generic_count += 1;
            if self.generic_count >= 1000 {
                self.generic_collect_count += self.generic_count;
                self.generic_count = 0;
                if self.generic_collect_count >= 10000 {
                    self.generic_collect_count = 0;
                    // SAFETY: This heap is exclusively borrowed by its owning thread.
                    unsafe { self.collect_all(false) };
                } else {
                    // SAFETY: This heap is exclusively borrowed by its owning thread.
                    unsafe { self.collect_retired(false) };
                }
            }
            // SAFETY: This heap is exclusively borrowed and find retains page ownership.
            let mut found = unsafe { self.find(request) };
            if found.is_none() {
                // SAFETY: No page was acquired. Forced collection frees unused
                // pages and purges backing before the native-style OOM retry.
                unsafe { self.collect_all(true) };
                // SAFETY: The heap is still exclusively borrowed.
                found = unsafe { self.find(request) };
            }
            let Some(found) = found else {
                return std::ptr::null_mut();
            };
            address = found;
        }
        // SAFETY: A queued/direct page remains exclusively owned by this heap.
        let data = unsafe { page(address) };
        // SAFETY: The selected free block covers the possibly overallocated request.
        let pointer = unsafe { pop::<ZEROED>(address) };
        // SAFETY: This heap retains ownership until abandonment below.
        let state = unsafe { local(address) };
        if (data.bin == HUGE_BIN || data.block_size > SMALL_MAX) && state.used == data.reserved {
            // SAFETY: This heap still owns the full queued page; the returned block pins it.
            unsafe { self.abandon(request.bin(), address) };
        }
        let mask = request.alignment - 1;
        let aligned = pointer.map_addr(|address| address.wrapping_add(mask) & !mask);
        if aligned != pointer {
            // The returned block pins the page even if it was abandoned above.
            // Owner transitions preserve this flag with a compare-exchange.
            data.owner.fetch_or(INTERIOR, Ordering::Relaxed);
        }
        aligned
    }

    /// # Safety
    /// The caller uniquely owns this live allocation and retires it exactly once.
    #[cfg(test)]
    #[inline]
    unsafe fn deallocate(&mut self, pointer: *mut u8) {
        // SAFETY: A live allocation has a registered page.
        let address = unsafe { pagemap::lookup(pointer.addr()) };
        // SAFETY: The live block pins the returned metadata.
        let data = unsafe { page(address) };
        let owner = data.owner.load(Ordering::Acquire);
        // The isolated test heap has no ThreadHeap fast path. Use the same
        // production transition that handles its aligned and remote blocks.
        // SAFETY: The live block pins the page and its acquired owner identity.
        unsafe { self.deallocate_generic(pointer, address, owner) };
    }

    /// # Safety
    /// The caller uniquely owns the live block and pins its page. `owner` is
    /// the page's acquired identity, including its interior-pointer flag.
    // Source free.c also outlines this path so ordinary local frees do not
    // spill registers for remote-list collection and arena reclamation.
    #[inline(never)]
    unsafe fn deallocate_generic(&mut self, pointer: *mut u8, address: usize, owner: usize) {
        // SAFETY: The still-live allocation pins this registered metadata.
        let data = unsafe { page(address) };
        let block = if owner & INTERIOR != 0 {
            // SAFETY: The live pointer belongs to this page's block area.
            unsafe { unalign(pointer, address) }
        } else {
            pointer.addr()
        };
        if owner & !OWNER_FLAGS == self.id {
            // SAFETY: The heap owns the page and retires this recovered base block once.
            if unsafe { free_local(address, block) } {
                // SAFETY: The page is still queued in the owning heap.
                unsafe { self.retire(data.bin, address) };
            }
            return;
        }
        // SAFETY: This block is live and is not owned by the current heap.
        unsafe { free_remote(address, block, Some(self)) };
    }
}

/// # Safety
/// This page has an interior-pointer flag, and the pointer is a live block
/// returned by this backend. The block pins immutable page geometry.
unsafe fn unalign(pointer: *mut u8, address: usize) -> usize {
    // SAFETY: The block pins initialized metadata.
    let data = unsafe { page(address) };
    let difference = pointer.addr() - data.start;
    pointer.addr() - difference % data.block_size
}

/// # Safety
/// The caller retires the block once. It is not in the supplied heap's queues.
/// A None heap denotes a thread that never initialized or already destroyed TLS.
unsafe fn free_remote(address: usize, block: usize, heap: Option<&mut Heap>) {
    // SAFETY: The still-live block pins this page.
    let data = unsafe { page(address) };
    let mut previous = data.remote.load(Ordering::Relaxed);
    loop {
        // SAFETY: The retiring block is exclusively owned until CAS publication.
        unsafe { set_next(block, previous & !OWNED) };
        match data
            .remote
            .compare_exchange_weak(previous, block | OWNED, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => break,
            Err(current) => previous = current,
        }
    }
    if previous & OWNED != 0 {
        return; // The owner can free the page immediately after publication.
    }
    let expected_head = if data.block_size <= 1024 {
        // SAFETY: The CAS published this head and acquired page ownership.
        unsafe { collect_partly(address, block) }
    } else {
        // SAFETY: The CAS acquired the abandoned page and its remote list.
        unsafe { collect(address) };
        0
    };
    // SAFETY: Acquired page ownership permits inspecting the used count.
    let used = unsafe { local(address) }.used;
    if used != 0
        && data.block_size <= MEDIUM_MAX
        && let Some(heap) = heap
        && heap.reclaim_on_free
        && data.original_owner.load(Ordering::Relaxed) == heap.id
        && (!heap.in_threadpool || heap.queues[data.bin].count <= 32)
    {
        // SAFETY: The page is abandoned and exclusively owned.
        unsafe { unmap_abandoned(address) };
        // SAFETY: The CAS transferred page ownership to this thread.
        unsafe { data.set_owner(heap.id) };
        // SAFETY: Reclamation permits detaching the remaining head.
        unsafe { collect(address) };
        // SAFETY: The page is no longer abandoned-indexed or locally queued.
        unsafe { heap.push(data.bin, address, true) };
    } else {
        // SAFETY: The page is abandoned and owned. Release handles pending frees.
        unsafe { release_abandoned(address, expected_head) };
    }
}

impl Heap {
    /// # Safety
    /// The heap and every queued page are exclusively owned for this call.
    unsafe fn finish(&mut self) {
        // SAFETY: Destruction exclusively owns this heap until every page is released.
        unsafe { self.finish_with(|_| {}) };
    }

    /// # Safety
    /// The heap is exclusively owned. The callback cannot retain page references
    /// or change page ownership.
    unsafe fn finish_with(&mut self, mut before_release: impl FnMut(usize)) {
        // Like theap.c's abandonment collect, expire empty retired pages and
        // clear retirement markers on reused pages before transferring ownership.
        // SAFETY: The heap owns its retirement queues.
        unsafe { self.collect_retired(true) };
        for bin in 0..BIN_COUNT {
            while self.queues[bin].first != 0 {
                let address = self.queues[bin].first;
                before_release(address);
                // SAFETY: Shutdown owns this page and merges its local lists.
                unsafe { collect_force(address) };
                // SAFETY: Every queued page belongs exclusively to this heap.
                unsafe { self.abandon(bin, address) };
            }
        }
    }
}

#[inline]
pub(super) fn allocate<const ZEROED: bool>(layout: Layout) -> *mut u8 {
    let pointer = STORAGE.with(|storage| {
        if storage.current.get().is_null() {
            return allocate_uninitialized::<ZEROED>(layout);
        }
        storage.thread.allocate::<ZEROED>(layout)
    });
    // Pair with the freeing thread's acquire fence. The allocator also supports
    // relaxed pointer-only handoffs that do not access application payloads.
    std::sync::atomic::fence(Ordering::Release);
    pointer
}

pub(super) fn allocate_regular(size: usize) -> *mut u8 {
    let Some(request) = Request::regular(size) else {
        return std::ptr::null_mut();
    };
    let pointer = with_heap(|heap| heap.allocate_request::<false>(request));
    std::sync::atomic::fence(Ordering::Release);
    pointer
}

/// # Safety
/// The pointer is a live allocation from this backend and is retired exactly once.
#[inline]
pub(super) unsafe fn deallocate(pointer: *mut u8) {
    std::sync::atomic::fence(Ordering::Acquire);
    STORAGE.with(|storage| {
        if storage.current.get().is_null() {
            // SAFETY: The caller transfers a live block on a thread without a heap.
            unsafe { deallocate_uninitialized(pointer) };
        } else {
            // SAFETY: The caller transfers the live allocation once.
            unsafe { storage.thread.deallocate(pointer) };
        }
    });
}

// Keep pointer recovery and remote collection off the ordinary local-free
// body. Otherwise Windows codegen reserves registers for the division here.
#[inline(never)]
unsafe fn deallocate_uninitialized(pointer: *mut u8) {
    // Free-only threads must not create an allocation heap. They can collect
    // abandoned pages but cannot reclaim them into thread-local queues.
    // SAFETY: The caller transfers a live registered allocation.
    let address = unsafe { pagemap::lookup(pointer.addr()) };
    // SAFETY: The block pins this initialized page.
    let data = unsafe { page(address) };
    let block = if data.owner.load(Ordering::Acquire) & INTERIOR == 0 {
        pointer.addr()
    } else {
        // SAFETY: The live pointer belongs to this page's block area.
        unsafe { unalign(pointer, address) }
    };
    // SAFETY: An uninitialized heap cannot own this block.
    unsafe { free_remote(address, block, None) };
}

/// # Safety
/// The pointer is a live allocation from this backend.
pub(super) unsafe fn usable_size(pointer: *mut u8) -> usize {
    // SAFETY: The live block pins its registered page.
    let address = unsafe { pagemap::lookup(pointer.addr()) };
    // SAFETY: The block keeps metadata and its immutable geometry live.
    let data = unsafe { page(address) };
    data.block_size - (pointer.addr() - data.start) % data.block_size
}

#[cfg(test)]
mod tests {
    use std::alloc::Layout;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    use super::{Heap, Request, STORAGE, allocate, collect_list, deallocate, extend, fresh, local, page, with_heap};

    #[test]
    fn observing_a_local_list_preserves_links_and_reports_budget_exhaustion() {
        let tail = 0usize;
        let head = std::ptr::from_ref(&tail).expose_provenance();
        let start = std::ptr::from_ref(&head).expose_provenance();

        // SAFETY: Both initialized stack words form an immutable local list.
        let partial = unsafe { super::observe_list(start, &mut 1) };
        // SAFETY: The same list remains live and unmodified for the complete visit.
        let complete = unsafe { super::observe_list(start, &mut 2) };

        assert_eq!((partial, complete, tail), ((1, false), (2, true), 0));
    }

    struct AfterHeap(Cell<Option<Arc<AtomicBool>>>);
    type LateAlignedResult = (bool, bool, [(usize, bool); 2]);
    struct AfterAlignedHeap(Cell<Option<mpsc::Sender<LateAlignedResult>>>);

    std::thread_local! {
        static AFTER_HEAP: AfterHeap = const { AfterHeap(Cell::new(None)) };
        static AFTER_ALIGNED_HEAP: AfterAlignedHeap = const { AfterAlignedHeap(Cell::new(None)) };
    }

    impl Drop for AfterHeap {
        #[cfg_attr(coverage_nightly, coverage(off))] // Test-only TLS destructor scaffolding.
        fn drop(&mut self) {
            if let Some(observed) = self.0.take() {
                observed.store(with_heap(|heap| !heap.reclaim_on_free), Ordering::Release);
            }
        }
    }

    impl Drop for AfterAlignedHeap {
        #[cfg_attr(coverage_nightly, coverage(off))] // Test-only TLS destructor scaffolding.
        fn drop(&mut self) {
            let Some(sender) = self.0.take() else {
                return;
            };
            let unavailable = super::HEAP_LIFETIME.try_with(|_| ()).is_err();
            let initially_clear = STORAGE.with(|storage| storage.current.get().is_null());
            let rejected = Layout::from_size_align(1, super::pagemap::REGION_SIZE).unwrap();
            assert_eq!(
                [allocate::<false>(rejected).is_null(), allocate::<true>(rejected).is_null()],
                [true, true]
            );
            let layout = Layout::from_size_align(256, 16).unwrap();
            let results = [false, true].map(|zeroed| {
                let pointer = if zeroed {
                    allocate::<true>(layout)
                } else {
                    allocate::<false>(layout)
                };
                if pointer.is_null() {
                    return (0, false);
                }
                // SAFETY: The successful allocation pins its immutable page geometry.
                let usable = unsafe { super::usable_size(pointer) };
                let zeroed_payload = !zeroed
                    || (0..layout.size()).all(|index| {
                        // SAFETY: alloc_zeroed initializes every byte of the live block.
                        unsafe { pointer.wrapping_add(index).read() == 0 }
                    });
                // SAFETY: This fixture owns and retires the successful allocation once.
                unsafe { deallocate(pointer) };
                (usable, zeroed_payload)
            });
            let remains_clear = STORAGE.with(|storage| storage.current.get().is_null());
            sender.send((unavailable, initially_clear && remains_clear, results)).unwrap();
        }
    }

    struct OwnedHeap(Heap);

    impl OwnedHeap {
        fn new() -> Self {
            let mut heap = Heap::new();
            heap.initialize();
            Self(heap)
        }
    }

    impl Drop for OwnedHeap {
        fn drop(&mut self) {
            // SAFETY: Each test exclusively owns its heap and ends all block access.
            unsafe { self.0.finish() };
        }
    }

    #[test]
    fn observation_without_an_initialized_heap_does_not_create_one() {
        let actual = std::thread::spawn(|| {
            let mut observed = seismograph_rallocator::mimalloc::Observation::EMPTY;
            let copied = super::observe_current(&mut observed);
            let initialized = STORAGE.with(|storage| !storage.current.get().is_null());
            (copied, initialized, observed)
        })
        .join()
        .unwrap();

        assert_eq!(actual, (false, false, seismograph_rallocator::mimalloc::Observation::EMPTY));
    }

    #[cfg_attr(
        miri,
        ignore = "Native VM backing is covered natively and under cargo careful; portable list readers run under Miri"
    )]
    #[test]
    fn exhausted_page_observation_preserves_allocator_queues() {
        let mut heap = OwnedHeap::new();
        let layout = Layout::from_size_align(64, 16).unwrap();
        let pointer = heap.0.allocate::<false>(layout);
        assert!(!pointer.is_null());
        let before = heap.0.queues.map(|queue| (queue.first, queue.last, queue.count));

        let mut result = seismograph_rallocator::mimalloc::Observation::EMPTY;
        heap.0.observe_with_budget(&mut result, 0);

        assert!(!result.pages_complete);
        assert!(
            result
                .classes
                .iter()
                .any(|class| class.queued_pages != 0 && class.observed_pages == 0)
        );
        assert_eq!(heap.0.queues.map(|queue| (queue.first, queue.last, queue.count)), before);
        // SAFETY: Observation leaves this checked allocation live in the same heap.
        unsafe { heap.0.deallocate(pointer) };
    }

    #[test]
    fn repeated_initialization_preserves_the_heap_identity() {
        let mut heap = OwnedHeap::new();
        let identity = heap.0.id;

        heap.0.initialize();

        assert_eq!(heap.0.id, identity);
    }

    #[test]
    fn invalid_regular_allocations_do_not_create_a_heap() {
        assert!(STORAGE.with(|storage| storage.current.get().is_null()));

        assert_eq!([0, usize::MAX].map(|size| super::allocate_regular(size).is_null()), [true, true]);

        assert!(STORAGE.with(|storage| storage.current.get().is_null()));
    }

    #[test]
    fn cold_entry_rejected_aligned_request_does_not_initialize_a_heap() {
        let actual = std::thread::spawn(|| {
            let layout = Layout::from_size_align(1, super::pagemap::REGION_SIZE).unwrap();

            let pointer = allocate::<false>(layout);
            let cached_is_null = STORAGE.with(|storage| storage.current.get().is_null());
            let identity = STORAGE.with(|storage| {
                // SAFETY: This thread is the sole user and no allocator call is active.
                unsafe { (*storage.thread.heap.get()).id }
            });

            (pointer.is_null(), cached_is_null, identity)
        })
        .join()
        .unwrap();

        assert_eq!(actual, (true, true, 0));
    }

    #[cfg_attr(
        miri,
        ignore = "Native VM reservations run natively and under cargo careful; portable bitmap readers retain Miri coverage"
    )]
    #[test]
    fn cold_entry_zeroed_request_keeps_payload_and_platform_selection() {
        let actual = std::thread::spawn(|| {
            let layout = Layout::from_size_align(64, 64).unwrap();

            let pointer = allocate::<true>(layout);
            assert!(!pointer.is_null());
            // SAFETY: The successful allocation pins its immutable page geometry.
            let usable = unsafe { super::usable_size(pointer) };
            let aligned = pointer.addr().is_multiple_of(64);
            let zeroed = (0..layout.size()).all(|index| {
                // SAFETY: alloc_zeroed initializes the complete live payload.
                unsafe { pointer.wrapping_add(index).read() == 0 }
            });
            // SAFETY: The allocating thread owns and retires this block exactly once.
            unsafe { deallocate(pointer) };
            (usable, aligned, zeroed)
        })
        .join()
        .unwrap();

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        assert_eq!(actual, (128, true, true));
        #[cfg(target_os = "linux")]
        assert_eq!(actual, (64, true, true));
    }

    #[cfg_attr(
        miri,
        ignore = "Native VM reservations run natively and under cargo careful; portable bitmap readers retain Miri coverage"
    )]
    #[test]
    fn cold_entry_late_aligned_requests_keep_the_non_null_empty_heap_path() {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            AFTER_ALIGNED_HEAP.with(|after| after.0.set(Some(sender)));
            let layout = Layout::from_size_align(256, 16).unwrap();
            let pointer = allocate::<false>(layout);
            assert!(!pointer.is_null());
            // SAFETY: This thread owns the live block and retires it exactly once.
            unsafe { deallocate(pointer) };
        })
        .join()
        .unwrap();

        assert_eq!(receiver.recv().unwrap(), (true, true, [(256, true), (256, true)]));
    }

    #[test]
    fn cached_tls_is_retired_before_late_heap_access() {
        let observed = Arc::new(AtomicBool::new(false));
        let late = Arc::clone(&observed);

        std::thread::spawn(move || {
            // Registration precedes the heap so this destructor runs after it.
            AFTER_HEAP.with(|after| after.0.set(Some(late)));
            let first = with_heap(|heap| heap.id);
            assert_eq!(with_heap(|heap| heap.id), first);
        })
        .join()
        .unwrap();

        assert!(observed.load(Ordering::Acquire));
    }

    #[cfg_attr(
        miri,
        ignore = "Native VM reservations run natively and under cargo careful; portable bitmap readers retain Miri coverage"
    )]
    #[test]
    fn a_full_page_cannot_extend_or_collect_an_empty_remote_list() {
        let mut heap = OwnedHeap::new();
        let request = Request::regular(64).unwrap();
        let address = fresh(request, heap.0.id).unwrap();
        // SAFETY: The fresh page belongs to this heap and is not queued.
        unsafe { heap.0.push(request.bin(), address, false) };
        // SAFETY: The heap owns the queued page and its immutable geometry.
        let count = unsafe { page(address) }.reserved;
        let blocks = (0..count).map(|_| heap.0.allocate_request::<false>(request)).collect::<Vec<_>>();
        assert!(blocks.iter().all(|block| !block.is_null()));

        // SAFETY: All initialized blocks are live, leaving both free lists empty.
        unsafe { extend(address) };
        // SAFETY: The page is owned and an empty detached list contains no blocks.
        unsafe { collect_list(address, 0) };
        // SAFETY: This test still owns the queued page and its local state.
        let state = unsafe { local(address) };
        assert_eq!((state.free, state.local_free, state.capacity, state.used), (0, 0, count, count));

        for block in blocks {
            // SAFETY: Each block is owned by this heap and retired exactly once.
            unsafe { heap.0.deallocate(block) };
        }
    }

    #[cfg_attr(
        miri,
        ignore = "Native VM reservations run natively and under cargo careful; portable bitmap readers retain Miri coverage"
    )]
    #[test]
    fn repeated_retirement_does_not_restart_the_expiration() {
        let mut heap = OwnedHeap::new();
        let request = Request::regular(64).unwrap();
        let block = heap.0.allocate_request::<false>(request);
        assert!(!block.is_null());
        // SAFETY: This live block pins its initialized metadata.
        let address = unsafe { super::pagemap::lookup(block.addr()) };
        // SAFETY: The owning heap retires this block exactly once.
        unsafe { heap.0.deallocate(block) };

        // SAFETY: The empty page remains in this heap's retired-page queue.
        unsafe { heap.0.retire(request.bin(), address) };

        // SAFETY: The retired page is still owned and queued.
        assert_eq!(unsafe { local(address) }.retire, 16);
    }

    #[cfg_attr(
        miri,
        ignore = "Native VM reservations run natively and under cargo careful; portable bitmap readers retain Miri coverage"
    )]
    #[test]
    fn shutdown_clears_reused_retirement_before_releasing_ownership() {
        let mut heap = OwnedHeap::new();
        let layout = Layout::from_size_align(64, 16).unwrap();
        let first = heap.0.allocate::<false>(layout);
        assert!(!first.is_null());
        // SAFETY: The owning heap retires this live allocation once.
        unsafe { heap.0.deallocate(first) };
        let live = heap.0.allocate::<false>(layout);
        assert!(!live.is_null());
        let mut visited = 0;
        let inspect = |address| {
            visited += 1;
            // SAFETY: Shutdown invokes this callback while it still owns the
            // queued page. Only the computed retirement state is read.
            assert_eq!(unsafe { local(address) }.retire, 0);
        };

        // SAFETY: The test exclusively owns the heap and the callback retains
        // neither references nor ownership of its page.
        unsafe { heap.0.finish_with(inspect) };

        assert_eq!(visited, 1);
        // SAFETY: The still-live allocation pins its now-abandoned page.
        unsafe { deallocate(live) };
    }

    #[cfg_attr(
        miri,
        ignore = "Native VM reservations run natively and under cargo careful; portable bitmap readers retain Miri coverage"
    )]
    #[test]
    fn free_only_consumers_do_not_initialize_a_thread_heap() {
        let layout = Layout::from_size_align(83, 16).unwrap();
        let pointer = allocate::<false>(layout);
        assert!(!pointer.is_null());
        let address = pointer.expose_provenance();

        let initialized = std::thread::spawn(move || {
            assert!(STORAGE.with(|storage| storage.current.get().is_null()));
            let pointer = std::ptr::with_exposed_provenance_mut::<u8>(address);
            // SAFETY: The producer transferred its live block only for deallocation.
            unsafe { deallocate(pointer) };
            STORAGE.with(|storage| !storage.current.get().is_null())
        })
        .join()
        .unwrap();

        assert!(!initialized);
    }

    #[cfg_attr(
        miri,
        ignore = "Native VM reservations run natively and under cargo careful; portable publication and bitmap tests retain Miri coverage"
    )]
    #[test]
    fn free_only_consumers_accept_interior_aligned_blocks_without_a_thread_heap() {
        let layout = Layout::from_size_align(83, 512).unwrap();
        let pointers = [allocate::<false>(layout), allocate::<false>(layout)];
        assert!(pointers.iter().all(|pointer| !pointer.is_null()));
        let addresses = pointers.map(<*mut u8>::expose_provenance);
        let has_interior_pointer = addresses.iter().any(|&pointer| {
            // SAFETY: Both allocations remain live and pin their page metadata.
            let address = unsafe { super::pagemap::lookup(pointer) };
            // SAFETY: The live block keeps this page's immutable geometry valid.
            let data = unsafe { page(address) };
            !(pointer - data.start).is_multiple_of(data.block_size)
        });
        assert!(has_interior_pointer, "the fixture must exercise an interior block pointer");

        let initialized = std::thread::spawn(move || {
            assert!(STORAGE.with(|storage| storage.current.get().is_null()));
            for address in addresses {
                let pointer = std::ptr::with_exposed_provenance_mut::<u8>(address);
                // SAFETY: The producer transfers both live aligned blocks exactly once.
                unsafe { deallocate(pointer) };
            }
            STORAGE.with(|storage| !storage.current.get().is_null())
        })
        .join()
        .unwrap();

        assert!(!initialized);
    }
}
