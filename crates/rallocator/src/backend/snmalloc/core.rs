// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Owner-exclusive frontend state. The endpoint's queue is outside the
//! `UnsafeCell`, so taking the core lease does not alias concurrent publishers.

use std::cell::UnsafeCell;
use std::sync::atomic::Ordering;

use crate::backend::snmalloc::classes::{self, CLASSES, COUNT, Request, SMALL_TAG};
use crate::backend::snmalloc::pagemap::{BACKEND, Map};
use crate::backend::snmalloc::ranges::Local;
use crate::backend::snmalloc::remote::{self, Cache, Queue};
use crate::sync_fence::fence;

#[repr(C, align(256))]
pub(crate) struct Owner {
    queue: Queue,
    state: UnsafeCell<Core>,
    pool_next: UnsafeCell<usize>,
    observation: crate::backend::snmalloc::observation::Metadata,
}

// SAFETY: The pool hands out exactly one core lease. Publishers access only the
// separately synchronized queue. Pool/inventory linkage and lease generations
// are pool-protected; observation slots are independently synchronized and do
// not let a collector borrow a leased core.
unsafe impl Sync for Owner {}

impl Owner {
    // Measured snmalloc 511e91a owner envelope: MSVC 14.51, x64, C++20,
    // non-hardened configuration. This is not a universal native type size.
    // Using Rust's smaller sizeof changes metadata placement and VM churn.
    pub(crate) const LOGICAL_SIZE: usize = 9216;
    pub(crate) const ALLOC_SIZE: usize = Self::LOGICAL_SIZE.next_power_of_two();
    pub(crate) const ALLOC_BITS: usize = Self::ALLOC_SIZE.trailing_zeros() as usize;
    const REALLOC_MAP_OFFSET: usize = Self::LOGICAL_SIZE - std::mem::size_of::<ReallocMap>();

    /// # Safety
    /// address identifies an exclusively owned, committed ALLOC_SIZE-byte
    /// metadata allocation with `ALLOC_SIZE` alignment, registered in map as
    /// backend-owned. The complete backing must remain live permanently.
    /// [0, `LOGICAL_SIZE`) is withheld, including the gap after the actual Owner
    /// and the realloc index stored at the end of that logical envelope;
    /// [`LOGICAL_SIZE`, `ALLOC_SIZE`) is transferred to the core's metadata buddy.
    /// After donation, the caller must not exclusively reborrow the entire
    /// backing, return it, or donate its tail again. The address must not contain
    /// an initialized Owner; pool reuse must not call this initializer.
    /// map must be `ranges::map()`'s handle, because both global refills and this
    /// frontend must use it.
    pub(crate) unsafe fn initialize(map: Map, address: usize) {
        let owner = address as *mut Self;
        let queue = Queue::new();
        // SAFETY: The supplied global map and persistent endpoint identify this core.
        let core = unsafe { Core::new(map, address) };
        // SAFETY: Caller provides stable, aligned storage for Owner and its
        // spare tail. Only the actual Owner bytes are initialized as Self.
        unsafe {
            owner.write(Self {
                queue,
                state: UnsafeCell::new(core),
                pool_next: UnsafeCell::new(0),
                observation: crate::backend::snmalloc::observation::Metadata::new(),
            });
        }
        // SAFETY: The sidecar occupies disjoint committed storage at the end
        // of the withheld logical owner envelope and is initialized once.
        unsafe { (address.wrapping_add(Self::REALLOC_MAP_OFFSET) as *mut ReallocMap).write(ReallocMap::new()) };
        // SAFETY: The freshly initialized Owner exclusively owns this core cell.
        let core = unsafe { (*owner).state.get() };
        // SAFETY: This initial lease covers only actual Core state.
        let core = unsafe { &mut *core };
        // The actual Owner, realloc sidecar and withheld gap never enter a buddy. Donation
        // is 1024@9216, 2048@10240, 4096@12288; neither owner references
        // nor core reborrows cover that independently managed suffix.
        // SAFETY: The disjoint suffix is committed, registered and donated once.
        unsafe {
            core.backend
                .seed_metadata(address + Self::LOGICAL_SIZE, Self::ALLOC_SIZE - Self::LOGICAL_SIZE);
        };
    }

    pub(crate) fn queue(&self) -> &Queue {
        &self.queue
    }

    /// Access still requires unsafe dereferencing and the unique owner lease.
    pub(crate) fn core_ptr(&self) -> *mut Core {
        self.state.get()
    }

    pub(crate) fn observation(&self) -> &crate::backend::snmalloc::observation::Metadata {
        &self.observation
    }

    /// # Safety
    /// Caller holds the endpoint-pool lock or owns an inactive endpoint.
    pub(crate) unsafe fn pool_next(&self) -> usize {
        // SAFETY: Pool linkage has no concurrent access under the caller's lock.
        unsafe { *self.pool_next.get() }
    }

    /// # Safety
    /// Caller holds the endpoint-pool lock or owns an inactive endpoint.
    /// next is zero or another valid inactive endpoint in the same pool;
    /// caller must preserve the pool's acyclic, uniquely linked FIFO.
    pub(crate) unsafe fn set_pool_next(&self, next: usize) {
        // SAFETY: Caller exclusively owns the pool linkage.
        unsafe { *self.pool_next.get() = next };
    }
}

const _: () = {
    // Preserve the original two-cache-line queue prefix and hot core placement.
    assert!(std::mem::offset_of!(Owner, state) == 128);
    assert!(std::mem::size_of::<Owner>() <= Owner::REALLOC_MAP_OFFSET);
    assert!(Owner::REALLOC_MAP_OFFSET.is_multiple_of(std::mem::align_of::<ReallocMap>()));
    assert!(std::mem::align_of::<Owner>() >= 256);
    assert!(Owner::ALLOC_SIZE.is_multiple_of(std::mem::align_of::<Owner>()));
    assert!(Owner::LOGICAL_SIZE < Owner::ALLOC_SIZE);
    assert!(Owner::LOGICAL_SIZE.is_multiple_of(16));
    assert!((Owner::ALLOC_SIZE - Owner::LOGICAL_SIZE).is_multiple_of(16));
};

#[repr(C)]
pub(crate) struct Slab {
    prev: usize,
    next: usize,
    free_head: usize,
    free_end: usize,
    base: usize,
    needed: u16,
    tag: u8,
    sleeping: bool,
}

impl Slab {
    pub(crate) const ALIGN: usize = std::mem::align_of::<Self>();
    const ALLOC_SIZE: usize = std::mem::size_of::<Self>().next_power_of_two();

    unsafe fn append(&mut self, first: usize, last: usize) {
        // A zero end denotes the head field without keeping a self-referential
        // raw pointer across Rust's exclusive metadata reborrows.
        if self.free_end == 0 {
            self.free_head = first;
        } else {
            // SAFETY: The end is the first word of an exclusively owned free
            // object, distinct from slab metadata and the incoming segment.
            unsafe { *(self.free_end as *mut usize) = first };
        }
        self.free_end = last;
    }

    unsafe fn close(&mut self) -> usize {
        // SAFETY: close is called only on a nonempty list; its last object is
        // exclusively owned and contains writable freelist storage.
        unsafe { *(self.free_end as *mut usize) = 0 };
        let first = self.free_head;
        self.free_head = 0;
        self.free_end = 0;
        first
    }
}

#[derive(Clone, Copy)]
struct List {
    head: usize,
}

impl List {
    const EMPTY: Self = Self { head: 0 };

    unsafe fn insert(&mut self, address: usize) {
        // SAFETY: The slab is stable, owner-exclusive, and not currently linked.
        let slab = unsafe { &mut *(address as *mut Slab) };
        slab.prev = 0;
        slab.next = self.head;
        if self.head != 0 {
            // SAFETY: The old head is a distinct slab owned by this list.
            unsafe { (*(self.head as *mut Slab)).prev = address };
        }
        self.head = address;
    }

    unsafe fn remove(&mut self, address: usize) {
        // SAFETY: This owner-exclusive slab is a member of this exact list.
        let slab = unsafe { &mut *(address as *mut Slab) };
        if slab.prev == 0 {
            self.head = slab.next;
        } else {
            // SAFETY: The previous link is a distinct member of this owned list.
            unsafe { (*(slab.prev as *mut Slab)).next = slab.next };
        }
        if slab.next != 0 {
            // SAFETY: The next link is a distinct member of this owned list.
            unsafe { (*(slab.next as *mut Slab)).prev = slab.prev };
        }
    }
}

#[derive(Clone, Copy)]
struct Available {
    slabs: List,
    length: usize,
    unused: usize,
}

impl Available {
    const EMPTY: Self = Self {
        slabs: List::EMPTY,
        length: 0,
        unused: 0,
    };
}

#[derive(Clone, Copy)]
struct LocalHint {
    base: usize,
    size: usize,
    meta: usize,
}

impl LocalHint {
    const EMPTY: Self = Self { base: 0, size: 0, meta: 0 };

    fn lookup(&mut self, ptr: usize) -> Option<usize> {
        // #[gamma::skip(logical.and_remove_left, literal.int_increment, reason = "A zero size makes the following unsigned range comparison false, while live slab sizes are whole chunks and never one")]
        if self.size != 0 && ptr.wrapping_sub(self.base) < self.size {
            return Some(self.meta);
        }
        if self.size != 0 {
            *self = Self::EMPTY;
        }
        None
    }

    fn observe(&mut self, base: usize, size: usize, meta: usize) {
        if self.size == 0 && self.meta == meta {
            *self = Self { base, size, meta };
        } else {
            *self = Self { base: 0, size: 0, meta };
        }
    }
}

#[derive(Clone, Copy)]
struct ReallocEntry {
    chunk: usize,
    meta: usize,
}

impl ReallocEntry {
    const EMPTY: Self = Self { chunk: 0, meta: 0 };
}

struct ReallocMap {
    entries: [ReallocEntry; 64],
}

impl ReallocMap {
    const fn new() -> Self {
        Self {
            entries: [ReallocEntry::EMPTY; 64],
        }
    }

    fn set(chunk: usize) -> usize {
        let chunk = chunk >> classes::CHUNK_BITS;
        ((chunk ^ (chunk >> 7)) & 31) * 2
    }

    fn insert(&mut self, base: usize, size: usize, meta: usize) {
        for chunk in (base..
            // #[gamma::skip(expr.decrement, reason = "This range starts on a chunk boundary and contains whole chunks, so subtracting one from its exclusive end retains every chunk start")]
            base + size)
            .step_by(classes::CHUNK)
        {
            let set = Self::set(chunk);
            let first = self.entries[set];
            let slot = if first.chunk == 0 || first.chunk == chunk { set } else { set + 1 };
            self.entries[slot] = ReallocEntry { chunk, meta };
        }
    }

    fn remove(&mut self, base: usize, size: usize, meta: usize) {
        for chunk in (base..
        // #[gamma::skip(arith.add_to_mul, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        // #[gamma::skip(expr.decrement, reason = "This range starts on a chunk boundary and contains whole chunks, so subtracting one from its exclusive end retains every chunk start")]
        base + size)
            .step_by(classes::CHUNK)
        {
            let set = Self::set(chunk);
            for entry in &mut self.entries[set..set + 2] {
                if (entry.chunk, entry.meta) == (chunk, meta) {
                    *entry = ReallocEntry::EMPTY;
                }
            }
        }
    }

    fn lookup(&self, ptr: usize) -> Option<usize> {
        let chunk = ptr & !(classes::CHUNK - 1);
        let set = Self::set(chunk);
        self.entries[set..set + 2]
            .iter()
            .find_map(|entry| (entry.chunk == chunk).then_some(entry.meta))
    }
}

pub(crate) struct Core {
    fast: [usize; COUNT],
    available: [Available; COUNT],
    laden: List,
    map: Map,
    owner: usize,
    backend: Local,
    remote: Cache,
    local_hint: LocalHint,
}

impl Core {
    pub(crate) fn observe(&self, budget: &mut usize) -> seismograph_rallocator::native::Observation {
        let mut result = seismograph_rallocator::native::Observation::EMPTY;
        for (index, state) in result.classes.iter_mut().enumerate() {
            let class = CLASSES[index];
            *state = seismograph_rallocator::native::ClassState {
                object_bytes: class.size as u64,
                slab_bytes: class.slab as u64,
                capacity: u64::from(class.capacity),
                available_slabs: self.available[index].length as u64,
                empty_slabs: self.available[index].unused as u64,
                observed_slabs: 0,
                fast_nonempty: self.fast[index] != 0,
            };
        }
        let heads = self
            .available
            .iter()
            .map(|available| available.slabs.head)
            .chain(std::iter::once(self.laden.head));
        for mut address in heads {
            while address != 0 {
                if *budget == 0 {
                    result.slabs_complete = false;
                    result.large.complete = false;
                    // #[gamma::skip(loop.break_to_continue, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                    break;
                }
                *budget -= 1;
                // SAFETY: The caller's existing lease or inactive-pool lock excludes
                // core/list mutation; metadata is owner-owned and remains committed.
                let slab = unsafe { &*(address as *const Slab) };
                let tag = usize::from(slab.tag);
                if tag >= SMALL_TAG {
                    result.classes[tag - SMALL_TAG].observed_slabs += 1;
                } else {
                    result.large.counts[64 - tag] += 1;
                }
                address = slab.next;
            }
        }
        result.local = self.backend.observe(budget);
        result.remote = self.remote.observe(self.map, budget);
        // SAFETY: Owner endpoints persist for process lifetime; the synchronized
        // queue is outside the core and is accessed exclusively through atomics.
        let (front, back) = unsafe { &*(self.owner as *const Owner) }.queue().observe();
        result.remote.incoming_front = front;
        result.remote.incoming_back = back;
        result
    }

    /// # Safety
    /// map is the backend's global map and owner is the stable endpoint being
    /// initialized around this core before any of its operations may run.
    unsafe fn new(map: Map, owner: usize) -> Self {
        Self {
            fast: [0; COUNT],
            available: [Available::EMPTY; COUNT],
            laden: List::EMPTY,
            map,
            owner,
            // SAFETY: Caller provides the backend's unique global map handle.
            backend: unsafe { Local::new(map) },
            remote: Cache::new(),
            local_hint: LocalHint::EMPTY,
        }
    }

    #[cfg(test)]
    pub(crate) fn map(&self) -> Map {
        self.map
    }

    fn realloc_map(&mut self) -> &mut ReallocMap {
        // SAFETY: The map occupies a disjoint, initialized portion of this
        // owner's withheld logical envelope and the Core lease is unique.
        unsafe { &mut *((self.owner + Owner::REALLOC_MAP_OFFSET) as *mut ReallocMap) }
    }

    /// `&mut self` is the unique owner lease, never shared with publishers.
    #[inline]
    pub(crate) fn allocate(&mut self, request: Request) -> *mut u8 {
        let tag = request.tag();
        if tag >= SMALL_TAG {
            let index = tag - SMALL_TAG;
            let head = self.fast[index];
            if head != 0 {
                // SAFETY: A hot-list node is exclusively owned, committed and
                // initialized. Taking it removes it before publishing to client.
                unsafe { self.fast[index] = *(head as *const usize) };
                // #[gamma::skip(stmt.delete_call, reason = "The non-faulting prefetch is a cache hint and changes neither the free list nor ownership")]
                crate::backend::snmalloc::hal::prefetch(self.fast[index]);
                return head as *mut u8;
            }
        }
        self.allocate_slow(request)
    }

    // The combined hot/refill body spilled eight callee-saved registers even
    // for a hot pop. Keep that refill frame off the owner allocation fast path.
    #[inline(never)]
    fn allocate_slow(&mut self, request: Request) -> *mut u8 {
        self.drain();
        let tag = request.tag();
        let ptr = if tag >= SMALL_TAG {
            self.refill(tag - SMALL_TAG)
        } else {
            self.new_slab(tag, request.size()) as *mut u8
        };
        publish_allocation(ptr)
    }

    fn new_slab(&mut self, tag: usize, size: usize) -> usize {
        let meta = self.backend.alloc_meta(Slab::ALLOC_SIZE);
        if meta == 0 {
            return 0;
        }
        let base = self.backend.alloc(size);
        if base == 0 {
            // SAFETY: Metadata was allocated but never initialized or published.
            unsafe { self.backend.free_meta(meta, Slab::ALLOC_SIZE) };
            return 0;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "Callers supply validated Request tags or 64 plus one of 44 class indices"
        )]
        let encoded_tag = tag as u8;
        let (needed, sleeping) = slab_initial_state(tag);
        // SAFETY: Independent metadata and object ranges are exclusively owned.
        // The stable slab is initialized before its pagemap is published.
        unsafe {
            (meta as *mut Slab).write(Slab {
                // #[gamma::skip(literal.int_increment, reason = "laden.insert below overwrites prev before publishing this exclusively owned new slab in its list")]
                prev: 0,
                // #[gamma::skip(literal.int_increment, reason = "laden.insert below overwrites next before publishing this exclusively owned new slab in its list")]
                next: 0,
                free_head: 0,
                free_end: 0,
                base,
                needed,
                tag: encoded_tag,
                sleeping,
            });
        }
        // SAFETY: The initialized slab and registered object range share this owner.
        unsafe {
            self.map.assign(
                base,
                // #[gamma::skip(expr.decrement, reason = "Pagemap assignment steps over whole chunks, so reducing the exclusive byte count by one retains the same final chunk start")]
                size,
                meta,
                self.owner | tag,
            );
        }
        self.local_hint.observe(
            base,
            // #[gamma::skip(expr.decrement, expr.increment, reason = "Fresh metadata differs from every live hint; release_slab clears matching hints before reuse, so this first observation records only meta and discards size")]
            size, meta,
        );
        let indexed_size = slab_indexed_size(
            // #[gamma::skip(expr.decrement, expr.increment, reason = "Valid large tags stay below SMALL_TAG after either one-step change; SMALL_TAG minus one still indexes the identical one-CHUNK first small slab")]
            tag,
            // #[gamma::skip(expr.decrement, reason = "Large tags ignore size; small whole-chunk sizes minus one visit exactly the same chunk starts in insert")]
            size,
        );
        self.realloc_map().insert(
            base,
            // #[gamma::skip(expr.decrement, reason = "The indexed extent is a nonzero whole number of chunks; reducing its exclusive end by one preserves every chunk start")]
            indexed_size,
            meta,
        );
        // SAFETY: This owner-exclusive slab is not yet linked in another list.
        unsafe { self.laden.insert(meta) };
        base
    }

    fn refill(&mut self, index: usize) -> *mut u8 {
        let class = CLASSES[index];
        let available = &mut self.available[index];
        let meta = available.slabs.head;
        let meta = if meta != 0 {
            // SAFETY: The available-list head belongs exclusively to this lease.
            unsafe { available.slabs.remove(meta) };
            available.length -= 1;
            // SAFETY: Removal does not retire the initialized slab.
            if unsafe { (*(meta as *const Slab)).needed } == 0 {
                available.unused -= 1;
            }
            // SAFETY: The slab is now unlinked and transfers to laden.
            unsafe { self.laden.insert(meta) };
            meta
        } else {
            let base = self.new_slab(SMALL_TAG + index, class.slab);
            if base == 0 {
                return std::ptr::null_mut();
            }
            // SAFETY: new_slab published this live frontend entry.
            let (meta, _) = unsafe { self.map.lookup(base) };
            // SAFETY: The new slab is exclusively owned by this lease.
            let slab = unsafe { &mut *(meta as *mut Slab) };
            // Non-hardened alloc_new_list: ascending complete-object starts.
            for i in 0..usize::from(class.capacity) {
                let ptr = base + i * class.size;
                // SAFETY: Each complete object is distinct and exclusively available.
                unsafe { slab.append(ptr, ptr) };
            }
            meta
        };
        // SAFETY: The selected slab belongs to this lease and has free objects.
        let slab = unsafe { &mut *(meta as *mut Slab) };
        // SAFETY: Its nonempty queue exclusively owns every linked free object.
        let first = unsafe { slab.close() };
        slab.sleeping = true;
        slab.needed = class.waking;
        // SAFETY: The first object contains an initialized freelist link.
        self.fast[index] = unsafe { *(first as *const usize) };
        // #[gamma::skip(stmt.delete_call, reason = "The non-faulting prefetch is a cache hint and changes neither the free list nor ownership")]
        crate::backend::snmalloc::hal::prefetch(self.fast[index]);
        first as *mut u8
    }

    /// # Safety
    /// ptr is the start of an exclusively owned live allocation in this map.
    pub(crate) unsafe fn deallocate(&mut self, ptr: usize) {
        if let Some(meta) = self.local_hint.lookup(
            // #[gamma::skip(expr.increment, reason = "Live allocation starts are at least sixteen-byte aligned with at least sixteen reserved bytes, so their next byte belongs to the identical live hint range")]
            ptr,
        ) {
            #[cfg(debug_assertions)]
            {
                // SAFETY: The live hint retains this initialized local slab.
                let tag = usize::from(unsafe { (*(meta as *const Slab)).tag });
                // SAFETY: ptr is the caller's live allocation.
                debug_assert_eq!(unsafe { self.map.lookup(ptr) }, (meta, self.owner | tag));
            }
            // SAFETY: The hint is cleared before slab retirement and identifies
            // only the current owner's live allocation range.
            unsafe { self.return_one(meta, ptr) };
            return;
        }
        // SAFETY: The caller transfers the live object into the outlined
        // cache/pagemap path exactly once.
        unsafe { self.deallocate_unhinted(ptr) };
    }

    #[inline(never)]
    unsafe fn deallocate_unhinted(&mut self, ptr: usize) {
        if let Some(meta) = self.realloc_map().lookup(
            // #[gamma::skip(expr.increment, reason = "Live allocation starts are at least sixteen-byte aligned, so adding one preserves their pagemap chunk key")]
            ptr,
        ) {
            // SAFETY: The local index retains this initialized local slab.
            let slab = unsafe { &*(meta as *const Slab) };
            let tag = usize::from(slab.tag);
            let size = slab_allocation_size(tag);
            self.local_hint.observe(slab.base, size, meta);
            #[cfg(debug_assertions)]
            {
                // SAFETY: The local index retains this initialized local slab.
                // SAFETY: ptr is the caller's live allocation.
                debug_assert_eq!(unsafe { self.map.lookup(ptr) }, (meta, self.owner | tag));
            }
            // SAFETY: The index contains only this owner's current live slabs.
            unsafe { self.return_one(meta, ptr) };
            return;
        }
        // Pair with a relaxed atomic pointer handoff before consulting the
        // address-indexed publication for a non-local allocation.
        let (meta, encoded) = lookup_published_with(
            // #[gamma::skip(expr.increment, reason = "A live allocation start is at least sixteen-byte aligned; its next byte has the identical pagemap chunk key")]
            ptr,
            |ptr| {
                // SAFETY: A live allocation guarantees its immutable frontend entry after acquisition.
                unsafe { self.map.lookup(ptr) }
            },
        );
        if encoded & !255 == self.owner {
            // SAFETY: The current pagemap snapshot identifies this live local slab.
            let slab = unsafe { &*(meta as *const Slab) };
            let size = slab_allocation_size(usize::from(slab.tag));
            self.local_hint.observe(slab.base, size, meta);
            // SAFETY: Ownership matches this lease; the object is now retired.
            unsafe { self.return_one(meta, ptr) };
        } else {
            // SAFETY: Caller transfers a live object belonging to another owner.
            unsafe { self.deallocate_remote(meta, ptr, encoded) };
        }
    }

    /// # Safety
    /// ptr is a live allocation from this allocator and `copy_len` fits both it
    /// and request.
    pub(crate) unsafe fn reallocate(&mut self, ptr: usize, request: Request, copy_len: usize) -> *mut u8 {
        let local = self.realloc_map().lookup(
            // #[gamma::skip(expr.increment, reason = "Live allocation starts are at least sixteen-byte aligned, so adding one preserves their pagemap chunk key")]
            ptr,
        );
        acquire_if_unindexed(local);
        let replacement = self.allocate(request);
        if replacement.is_null() {
            return replacement;
        }
        // SAFETY: Caller bounds copy_len to both disjoint live allocations.
        unsafe { std::ptr::copy_nonoverlapping(ptr as *const u8, replacement, copy_len) };
        if let Some(meta) = local {
            #[cfg(debug_assertions)]
            {
                // SAFETY: The realloc index retains this initialized local slab.
                let tag = usize::from(unsafe { (*(meta as *const Slab)).tag });
                // SAFETY: ptr remains live until return_one below.
                debug_assert_eq!(unsafe { self.map.lookup(ptr) }, (meta, self.owner | tag));
            }
            // SAFETY: The realloc index contains only this owner's live slabs.
            unsafe { self.return_one(meta, ptr) };
        } else {
            // SAFETY: The original allocation remains live after copying.
            unsafe { self.deallocate(ptr) };
        }
        replacement
    }

    /// # Safety
    /// ptr is a live allocation, not concurrently freed.
    #[cfg(test)]
    pub(crate) unsafe fn usable_size(&mut self, ptr: usize) -> usize {
        if let Some(meta) = self.realloc_map().lookup(ptr) {
            // SAFETY: The local index retains this initialized local slab.
            return classes::size(usize::from(unsafe { (*(meta as *const Slab)).tag }));
        }
        // Pair with a relaxed atomic pointer handoff before reading a foreign
        // allocation's immutable pagemap publication.
        fence(Ordering::Acquire);
        // SAFETY: The caller guarantees immutable live frontend metadata.
        let (_, encoded) = unsafe { self.map.lookup(ptr) };
        classes::size(encoded & 127)
    }

    // Isolate remote-cache register pressure from the common local return.
    #[inline(never)]
    unsafe fn deallocate_remote(&mut self, meta: usize, ptr: usize, encoded: usize) {
        let fits = self.remote.reserve(classes::size(encoded & 127));
        // SAFETY: Cache takes exclusive ownership while retaining the slab.
        unsafe { self.remote.deallocate(self.map, meta, ptr) };
        if !fits {
            // SAFETY: All cached messages target other stable endpoints.
            unsafe { self.remote.post(self.map, self.owner) };
        }
    }

    unsafe fn return_one(&mut self, meta: usize, ptr: usize) {
        // SAFETY: The owner exclusively holds the slab and this single returned
        // object. Append order and counter transitions are identical to a batch
        // of one, without passing a fifth argument or entering the batch frame.
        let slab = unsafe { &mut *(meta as *mut Slab) };
        // SAFETY: This retired object transfers exactly once to the slab's queue.
        unsafe { slab.append(ptr, ptr) };
        // #[gamma::skip(cond.always_false, relational.gt_to_lt, literal.int_increment, reason = "A live object's slab has a positive needed threshold; for needed greater than one return_count(meta,1) takes its first branch and performs the identical subtraction, while needed one uses the unchanged transition branch")]
        if slab.needed > 1 {
            slab.needed -= 1;
        } else {
            // SAFETY: Exactly one appended object reaches the next transition.
            unsafe { self.return_count(meta, 1) };
        }
    }

    unsafe fn return_local(&mut self, meta: usize, first: usize, last: usize, count: u16) {
        // SAFETY: This lease owns the slab and the exact incoming segment.
        let slab = unsafe { &mut *(meta as *mut Slab) };
        // SAFETY: The exact exclusively owned segment is appended once.
        unsafe { slab.append(first, last) };
        // SAFETY: count describes the segment just transferred to this slab.
        unsafe { self.return_count(meta, count) };
    }

    // The former general return frame spilled XMM6 for every local free.
    // Only counter-boundary transitions and batched returns need this frame.
    #[inline(never)]
    unsafe fn return_count(&mut self, meta: usize, mut count: u16) {
        loop {
            // SAFETY: This lease owns the slab and the exact returned segment.
            let slab = unsafe { &mut *(meta as *mut Slab) };
            if count < slab.needed {
                slab.needed -= count;
                break;
            }
            count -= slab.needed;
            slab.needed = 0;
            let tag = usize::from(slab.tag);
            if tag < SMALL_TAG {
                debug_assert_eq!(count, 0);
                // SAFETY: This fully returned large slab is a laden member.
                unsafe { self.laden.remove(meta) };
                // SAFETY: The unlinked slab has no outstanding objects.
                unsafe { self.release_slab(meta) };
                break;
            }
            let index = tag - SMALL_TAG;
            if
            // #[gamma::skip(cond.always_true, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            slab.sleeping {
                // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                slab.sleeping =
                // #[gamma::skip(literal.bool_flip, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                false;
                slab.needed = CLASSES[index].capacity - CLASSES[index].waking;
                // SAFETY: The sleeping slab is linked only in laden.
                unsafe { self.laden.remove(meta) };
                // SAFETY: The now-unlinked slab transfers to its available list.
                unsafe { self.available[index].slabs.insert(meta) };
                self.available[index].length += 1;
            } else {
                self.available[index].unused += 1;
                if self.available[index].unused > 2 && self.available[index].unused > self.available[index].length / 4 {
                    self.reclaim(index);
                }
                // No more objects can be outstanding when a slab is empty.
                debug_assert_eq!(count, 0);
                // #[gamma::skip(loop.break_to_continue, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                // #[gamma::skip(loop.delete_break, reason = "An empty slab has no outstanding objects, so count is zero; removing this break immediately reaches the unchanged count-zero break below without another state transition")]
                break;
            }
            // #[gamma::skip(cond.always_false, loop.break_to_continue, loop.delete_break, reason = "This check is reached only after waking a small slab, whose capacity exceeds waking; for count zero the next iteration subtracts zero from its positive needed count and immediately breaks, changing no list or counter")]
            if count == 0 {
                break;
            }
        }
    }

    unsafe fn release_slab(&mut self, meta: usize) {
        // SAFETY: Slab has been unlinked and all objects have returned, including
        // the queue's retained tail. No frontend readers can remain.
        let slab = unsafe { &*(meta as *const Slab) };
        let base = slab.base;
        let tag = usize::from(slab.tag);
        let size = slab_allocation_size(tag);
        if self.local_hint.meta == meta {
            self.local_hint = LocalHint::EMPTY;
        }
        let indexed_size = slab_indexed_size(
            // #[gamma::skip(expr.decrement, expr.increment, reason = "Valid large tags stay below SMALL_TAG after either one-step change; SMALL_TAG minus one still indexes the identical one-CHUNK first small slab")]
            tag,
            // #[gamma::skip(expr.decrement, expr.increment, reason = "Large tags ignore size; small size minus one visits the same chunks, while plus one only visits an adjacent chunk that cannot share this live slab's unique metadata")]
            size,
        );
        self.realloc_map().remove(
            base,
            // #[gamma::skip(expr.decrement, expr.increment, reason = "Minus one keeps every indexed chunk; plus one only visits an adjacent chunk, and remove requires this retiring slab's exact unique metadata before changing an entry")]
            indexed_size,
            meta,
        );
        // SAFETY: No frontend reader remains for these registered entries.
        unsafe {
            self.map.assign(
                // #[gamma::skip(expr.increment, reason = "This chunk-aligned range still indexes the identical pagemap entries when each stepped address is increased by one")]
                base,
                // #[gamma::skip(expr.decrement, reason = "The retired range is a nonzero whole number of chunks; reducing its exclusive end by one preserves every assigned entry")]
                size, 0, BACKEND,
            );
        }
        // SAFETY: The slab is unlinked and its metadata is no longer used.
        unsafe { self.backend.free_meta(meta, Slab::ALLOC_SIZE) };
        // SAFETY: All objects returned and the map now marks backend ownership.
        unsafe { self.backend.free(base, size) };
    }

    fn reclaim(&mut self, index: usize) {
        let mut meta = self.available[index].slabs.head;
        while meta != 0 {
            // SAFETY: Every available-list slab is stable and owner-exclusive.
            let slab = unsafe { &*(meta as *const Slab) };
            let next = slab.next;
            if slab.needed == 0 {
                // SAFETY: This slab belongs to this exact available list.
                unsafe { self.available[index].slabs.remove(meta) };
                self.available[index].length -= 1;
                self.available[index].unused -= 1;
                // SAFETY: needed==0 proves no hot or remotely retained object remains.
                unsafe { self.release_slab(meta) };
            }
            meta = next;
        }
    }

    fn drain(&mut self) {
        // SAFETY: The endpoint is persistent and this core is its unique lease;
        // the queue is disjoint from mutable core state (an UnsafeCell).
        let queue = unsafe { (*(self.owner as *const Owner)).queue() };
        // #[gamma::skip(literal.int_increment, reason = "Every drained byte amount and DRAIN_LIMIT is a multiple of the sixteen-byte minimum class; a one-byte budget offset crosses no additional threshold")]
        let mut bytes = 0usize;
        let mut post = false;
        let consume = |message| {
            // SAFETY: Queue drain hands over a live message and its retained slab.
            let (meta, encoded) = unsafe { self.map.lookup(message) };
            // SAFETY: The dequeued message's initialized ring is exclusively owned.
            let (first, count) = unsafe { remote::open(message) };
            let amount = classes::size(encoded & 127) * usize::from(count);
            bytes += amount;
            if encoded & !255 == self.owner {
                // SAFETY: The owner matches and the exact ring transfers once.
                unsafe { self.return_local(meta, first, message, count) };
            } else {
                // #[gamma::skip(logical.and_remove_left, reason = "Once post is true, extra reserve calls only alter the private remaining budget that the guaranteed post below resets to BUDGET")]
                if !post && !self.remote.reserve(amount) {
                    post = true;
                }
                // SAFETY: This initialized remote message transfers to the outgoing cache.
                unsafe { self.remote.forward(self.map, message, 0) };
            }
            bytes < remote::DRAIN_LIMIT
        };
        // SAFETY: This is the only consumer; consume cannot recursively drain this queue.
        unsafe { queue.drain(consume) };
        if post {
            // SAFETY: The cache contains exclusively owned messages for other endpoints.
            unsafe { self.remote.post(self.map, self.owner) };
        }
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.fast.iter().all(|&p| p == 0)
            && self
                .available
                .iter()
                .all(|entry| entry.slabs.head == 0 && entry.length == 0 && entry.unused == 0)
            && self.laden.head == 0
    }

    #[cfg(test)]
    pub(crate) fn outstanding_objects(&self) -> usize {
        let mut total = 0;
        for head in self
            .available
            .iter()
            .map(|entry| entry.slabs.head)
            .chain(std::iter::once(self.laden.head))
        {
            let mut address = head;
            while address != 0 {
                // SAFETY: The test holds the core lease throughout traversal.
                let slab = unsafe { &*(address as *const Slab) };
                total += usize::from(slab.needed);
                if slab.sleeping {
                    let class = CLASSES[usize::from(slab.tag) - SMALL_TAG];
                    total += usize::from(class.capacity - class.waking);
                }
                address = slab.next;
            }
        }
        total
    }

    #[cfg(test)]
    #[expect(
        clippy::too_many_lines,
        reason = "one audit pass keeps cross-structure invariants visible together"
    )]
    fn validate(&self, live: &std::collections::BTreeSet<usize>) {
        use std::collections::{BTreeMap, BTreeSet};

        let mut slabs = BTreeMap::new();
        let mut listed = BTreeSet::new();
        for (expected_index, head) in self
            .available
            .iter()
            .enumerate()
            .map(|(index, entry)| (Some(index), entry.slabs.head))
            .chain(std::iter::once((None, self.laden.head)))
        {
            let mut previous = 0;
            let mut meta = head;
            while meta != 0 {
                assert!(listed.insert(meta), "slab {meta:#x} is linked more than once");
                // SAFETY: This test owns the quiescent core lease and every
                // linked slab remains initialized until it is unlinked.
                let slab = unsafe { &*(meta as *const Slab) };
                assert_eq!(slab.prev, previous, "broken previous link at slab {meta:#x}");
                let tag = usize::from(slab.tag);
                if let Some(index) = expected_index {
                    assert_eq!(tag, SMALL_TAG + index, "slab appears in the wrong available list");
                    assert!(!slab.sleeping, "available slab is still marked sleeping");
                } else if tag >= SMALL_TAG && slab.sleeping {
                    assert!(slab.needed > 0, "sleeping laden slab has no outstanding threshold");
                }
                let size = if tag >= SMALL_TAG {
                    CLASSES[tag - SMALL_TAG].slab
                } else {
                    classes::size(tag)
                };
                assert_eq!(slab.base % size, 0, "slab base is not naturally aligned");
                for offset in (0..size).step_by(classes::CHUNK) {
                    // SAFETY: A linked slab retains immutable frontend entries.
                    let actual = unsafe { self.map.lookup(slab.base + offset) };
                    assert_eq!(actual, (meta, self.owner | tag), "pagemap disagrees with slab {meta:#x}");
                }
                slabs.insert(meta, (slab.base, tag, size));
                previous = meta;
                meta = slab.next;
            }
        }
        for (index, available) in self.available.iter().enumerate() {
            let actual = slabs
                .iter()
                .filter(|(meta, (_, tag, _))| {
                    // SAFETY: Every collected metadata address remains linked.
                    *tag == SMALL_TAG + index && !unsafe { (**meta as *const Slab).as_ref().unwrap().sleeping }
                })
                .count();
            assert_eq!(actual, available.length, "available slab count mismatch for class {index}");
            let unused = slabs
                .iter()
                .filter(|(meta, (_, tag, _))| {
                    // SAFETY: Every collected metadata address remains linked.
                    *tag == SMALL_TAG + index && unsafe { (**meta as *const Slab).as_ref().unwrap().needed == 0 }
                })
                .count();
            assert_eq!(unused, available.unused, "unused slab count mismatch for class {index}");
        }

        let mut free = BTreeSet::new();
        let mut free_per_slab = BTreeMap::<usize, usize>::new();
        for (index, &head) in self.fast.iter().enumerate() {
            let mut ptr = head;
            while ptr != 0 {
                assert!(free.insert(ptr), "free object {ptr:#x} appears more than once");
                // SAFETY: Every hot-list object retains immutable slab metadata.
                let (meta, encoded) = unsafe { self.map.lookup(ptr) };
                let &(base, tag, size) = slabs.get(&meta).unwrap();
                assert_eq!(tag, SMALL_TAG + index, "hot object is in the wrong size class");
                let class = CLASSES[index];
                assert!(ptr >= base && ptr < base + size && (ptr - base).is_multiple_of(class.size));
                assert_eq!(encoded, self.owner | tag);
                *free_per_slab.entry(meta).or_default() += 1;
                // SAFETY: A hot-list object exclusively owns its first link word.
                ptr = unsafe { *(ptr as *const usize) };
            }
        }
        for (&meta, &(base, tag, size)) in &slabs {
            // SAFETY: The collected slab remains linked and initialized.
            let slab = unsafe { &*(meta as *const Slab) };
            let mut ptr = slab.free_head;
            if ptr == 0 {
                assert_eq!(slab.free_end, 0);
                continue;
            }
            assert_ne!(slab.free_end, 0);
            loop {
                assert!(free.insert(ptr), "free object {ptr:#x} appears more than once");
                let object_size = classes::size(tag);
                assert!(ptr >= base && ptr < base + size && (ptr - base).is_multiple_of(object_size));
                *free_per_slab.entry(meta).or_default() += 1;
                if ptr == slab.free_end {
                    break;
                }
                // SAFETY: Non-tail free objects contain an initialized next link.
                ptr = unsafe { *(ptr as *const usize) };
                assert_ne!(ptr, 0, "slab free queue ended before its recorded tail");
            }
        }
        assert!(live.is_disjoint(&free), "a live allocation is present in a free structure");
        let mut live_per_slab = BTreeMap::<usize, usize>::new();
        for &ptr in live {
            // SAFETY: The test's live set contains only allocations from this core.
            let (meta, encoded) = unsafe { self.map.lookup(ptr) };
            let &(base, tag, size) = slabs.get(&meta).unwrap();
            let object_size = classes::size(tag);
            assert!(ptr >= base && ptr < base + size && (ptr - base).is_multiple_of(object_size));
            assert_eq!(encoded, self.owner | tag);
            *live_per_slab.entry(meta).or_default() += 1;
        }
        for (&meta, &(base, tag, size)) in &slabs {
            let capacity = if tag >= SMALL_TAG {
                usize::from(CLASSES[tag - SMALL_TAG].capacity)
            } else {
                1
            };
            assert_eq!(
                free_per_slab.get(&meta).copied().unwrap_or(0) + live_per_slab.get(&meta).copied().unwrap_or(0),
                capacity,
                "slab {meta:#x} does not partition into live and free objects"
            );
            if self.local_hint.meta == meta && self.local_hint.size != 0 {
                assert_eq!((self.local_hint.base, self.local_hint.size), (base, size));
            }
        }
        assert!(self.remote.is_empty(), "single-owner validation encountered pending remote state");
        self.backend.validate();
    }

    pub(crate) fn flush(&mut self) {
        // SAFETY: Endpoint is stable and this core is its unique consumer lease.
        let queue = unsafe { (*(self.owner as *const Owner)).queue() };
        // Preserve native flush's retained user tail, including at owner exit.
        // Advancing it with a dummy message would change slab/OS retention.
        while
        // #[gamma::skip(cond.negate, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        queue.can_dequeue() {
            // #[gamma::skip(stmt.delete_call, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            self.drain();
        }
        for index in 0..COUNT {
            while self.fast[index] != 0 {
                let ptr = self.fast[index];
                // SAFETY: Hot-list elements are exclusively owned and remain
                // counted as outstanding until individually returned here.
                self.fast[index] = unsafe { *(ptr as *const usize) };
                // SAFETY: The just-removed hot object still retains its slab.
                let (meta, _) = unsafe { self.map.lookup(ptr) };
                // SAFETY: This owner's hot object is returned exactly once.
                unsafe { self.return_one(meta, ptr) };
            }
        }
        // SAFETY: Outgoing cache is exclusively owned and targets other owners.
        unsafe { self.remote.post(self.map, self.owner) };
        for index in 0..COUNT {
            self.reclaim(index);
        }
        // Native flush forces the next remote return through its posting path.
        self.remote.clear_budget();
    }
}

#[inline]
fn publish_allocation(ptr: *mut u8) -> *mut u8 {
    if !ptr.is_null() {
        fence(Ordering::Release);
    }
    ptr
}

fn slab_initial_state(tag: usize) -> (u16, bool) {
    (if tag >= SMALL_TAG { CLASSES[tag - SMALL_TAG].waking } else { 1 }, tag >= SMALL_TAG)
}

fn slab_allocation_size(tag: usize) -> usize {
    if tag >= SMALL_TAG {
        CLASSES[tag - SMALL_TAG].slab
    } else {
        classes::size(tag)
    }
}

fn slab_indexed_size(tag: usize, size: usize) -> usize {
    // #[gamma::skip(relational.ge_to_gt, reason = "At SMALL_TAG the first small slab is exactly CHUNK bytes, so both indexed-size branches are identical")]
    if tag >= SMALL_TAG { size } else { classes::CHUNK }
}

#[inline]
fn lookup_published_with<T>(ptr: usize, lookup: impl FnOnce(usize) -> T) -> T {
    fence(Ordering::Acquire);
    lookup(ptr)
}

#[inline]
fn acquire_if_unindexed(local: Option<usize>) {
    if local.is_none() {
        fence(Ordering::Acquire);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn nov6_unindexed_reallocation_acquires_before_allocating_or_copying() {
        let address = fresh_owner();
        // SAFETY: The fresh persistent owner belongs exclusively to this test.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: This test holds the owner's only core lease.
        let core = unsafe { &mut *owner.core_ptr() };
        let request = Request::new(std::alloc::Layout::from_size_align(16, 16).unwrap()).unwrap();
        let original = core.allocate(request);
        assert!(!original.is_null());
        // SAFETY: This test exclusively owns the complete sixteen-byte allocation.
        unsafe { original.write_bytes(0x5a, 16) };
        // SAFETY: The live allocation retains its initialized immutable metadata.
        let meta = unsafe { core.map.lookup(original.addr()) }.0;
        // SAFETY: The current core exclusively owns the live slab metadata.
        let base = unsafe { (*(meta as *const Slab)).base };
        core.realloc_map().remove(base, CLASSES[0].slab, meta);
        core.local_hint = LocalHint::EMPTY;
        assert_ne!(core.fast[0], 0);
        let trace = crate::sync_fence::trace::Trace::start();
        // SAFETY: Both old and requested layouts contain the entire copied sixteen-byte range.
        let replacement = unsafe { core.reallocate(original.addr(), request, 16) };
        assert!(!replacement.is_null());
        // The second acquire belongs to deallocation and cannot replace the
        // handoff fence before the hot allocation and copy.
        assert_eq!(trace.events(), [Ordering::Acquire, Ordering::Acquire]);
        drop(trace);
        // SAFETY: The replacement exclusively owns sixteen initialized copied bytes.
        assert_eq!(unsafe { std::slice::from_raw_parts(replacement, 16) }, [0x5a; 16]);
        // SAFETY: This is the unique return of the replacement allocation.
        unsafe { core.deallocate(replacement.addr()) };
        core.flush();
        assert!(core.is_empty());
    }

    #[test]
    fn nov6_new_slab_initializes_threshold_before_refill_and_retires_every_frontend_entry() {
        let address = fresh_owner();
        // SAFETY: The fresh persistent owner belongs exclusively to this test.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: This test holds the owner's only core lease.
        let core = unsafe { &mut *owner.core_ptr() };
        for index in [0, 1, COUNT - 1] {
            let class = CLASSES[index];
            let base = core.new_slab(SMALL_TAG + index, class.slab);
            assert_ne!(base, 0);
            // SAFETY: new_slab initialized the registered entries and retains their backing.
            let meta = unsafe { core.map.lookup(base) }.0;
            // SAFETY: The fresh slab is stable and belongs exclusively to this core.
            let slab = unsafe { &*(meta as *const Slab) };
            assert_eq!(slab.needed, class.waking);
            assert!(slab.sleeping);
            for offset in (0..class.slab).step_by(classes::CHUNK) {
                assert_eq!(core.realloc_map().lookup(base + offset), Some(meta));
            }
            assert_eq!(core.realloc_map().lookup(base + class.slab), None);
            // SAFETY: The fresh slab belongs to this core and has exactly one list link.
            unsafe { core.laden.remove(meta) };
            // SAFETY: The slab is unlinked and no allocation was issued from it.
            unsafe { core.release_slab(meta) };
            for offset in (0..class.slab).step_by(classes::CHUNK) {
                assert_eq!(core.realloc_map().lookup(base + offset), None);
                // SAFETY: The map itself remains committed; retirement must leave initialized backend entries.
                assert_eq!(unsafe { core.map.lookup(base + offset) }, (0, BACKEND));
            }
        }
    }

    #[test]
    fn gamma_nov4_slab_publication_and_retirement_keep_exact_thresholds_and_full_class_ranges() {
        for (index, class) in CLASSES.iter().enumerate() {
            let tag = SMALL_TAG + index;
            assert_eq!(slab_initial_state(tag), ((class.capacity / 4).min(32), true));
            assert_eq!(slab_allocation_size(tag), class.slab);
            assert_eq!(slab_indexed_size(tag, class.slab), class.slab);
        }
        for bits in [17, 18, 19, 30, 62] {
            let tag = 64 - bits;
            let size = 1usize << bits;
            assert_eq!(slab_initial_state(tag), (1, false));
            assert_eq!(slab_allocation_size(tag), size);
            assert_eq!(slab_indexed_size(tag, size), 16384);
        }
    }

    #[test]
    fn gamma_nov4_publication_and_lookup_follow_exact_success_failure_and_indexed_handoff_orders() {
        use crate::sync_fence::trace::Trace;
        let mut byte = 71u8;
        let pointer = &raw mut byte;
        {
            let trace = Trace::start();
            assert!(publish_allocation(std::ptr::null_mut()).is_null());
            assert!(trace.events().is_empty());
            assert_eq!(publish_allocation(pointer), pointer);
            assert_eq!(trace.events(), [Ordering::Release]);
            let result = lookup_published_with(pointer.addr(), |address| {
                assert_eq!(address, pointer.addr());
                assert_eq!(trace.events(), [Ordering::Release, Ordering::Acquire]);
                (17, 29)
            });
            assert_eq!(result, (17, 29));
        }
        for indexed in [false, true] {
            let trace = Trace::start();
            acquire_if_unindexed(indexed.then_some(17usize));
            assert_eq!(trace.events(), if indexed { vec![] } else { vec![Ordering::Acquire] });
        }
    }

    #[test]
    fn gamma_round2_slow_allocation_drains_existing_remote_returns_before_refilling() {
        let target = fresh_owner();
        let sender = fresh_owner();
        // SAFETY: the two persistent endpoints are uniquely leased by this test.
        let target_owner = unsafe { &*(target as *const Owner) };
        // SAFETY: this test is the target's only core borrower.
        let target_core = unsafe { &mut *target_owner.core_ptr() };
        // SAFETY: sender is a separate persistent endpoint.
        let sender_owner = unsafe { &*(sender as *const Owner) };
        // SAFETY: this test uniquely leases the sender's core.
        let sender_core = unsafe { &mut *sender_owner.core_ptr() };
        let small = Request::new(std::alloc::Layout::from_size_align(32, 16).unwrap()).unwrap();
        for _ in 0..3 {
            for _ in 0..2 {
                let pointer = target_core.allocate(small);
                assert!(!pointer.is_null());
                // SAFETY: the live target allocation is transferred once to the sender.
                unsafe { sender_core.deallocate(pointer.addr()) };
            }
            // SAFETY: all cached objects target the other persistent owner.
            unsafe { sender_core.remote.post(sender_core.map, sender) };
        }
        let (first, last) = target_owner.queue().observe();
        assert_ne!(first, last, "the fixture must contain consumable messages before allocation");
        let request = Request::new(std::alloc::Layout::from_size_align(96, 16).unwrap()).unwrap();
        let pointer = target_core.allocate(request);
        assert!(!pointer.is_null());
        assert_eq!(target_owner.queue().observe(), (last, last));
        // SAFETY: the unrelated allocation is returned once to its owner.
        unsafe { target_core.deallocate(pointer.addr()) };
        sender_core.flush();
        target_core.flush();
    }

    #[test]
    fn gamma_round2_remote_cache_keeps_exact_metadata_and_forwarded_round_zero_routing() {
        let target = fresh_owner();
        let sender = fresh_owner();
        let forwarder = fresh_owner();
        // SAFETY: this test owns three distinct persistent endpoints and their sole core leases.
        let target_owner = unsafe { &*(target as *const Owner) };
        // SAFETY: the target core is exclusively owned by this test.
        let target_core = unsafe { &mut *target_owner.core_ptr() };
        // SAFETY: the sender is a separate exclusively leased persistent endpoint.
        let sender_owner = unsafe { &*(sender as *const Owner) };
        // SAFETY: the sender core is exclusively owned by this test.
        let sender_core = unsafe { &mut *sender_owner.core_ptr() };
        // SAFETY: the forwarder is a separate exclusively leased persistent endpoint.
        let forwarder_owner = unsafe { &*(forwarder as *const Owner) };
        // SAFETY: the forwarder core is exclusively owned by this test.
        let forwarder_core = unsafe { &mut *forwarder_owner.core_ptr() };
        let request = Request::new(std::alloc::Layout::from_size_align(32, 16).unwrap()).unwrap();
        let pointers = (0..6).map(|_| target_core.allocate(request)).collect::<Vec<_>>();
        assert!(pointers.iter().all(|pointer| !pointer.is_null()));
        for pair in pointers.chunks_exact(2) {
            for pointer in pair {
                // SAFETY: each distinct live target allocation is transferred once to the sender.
                unsafe { sender_core.deallocate(pointer.addr()) };
                sender_core.remote.assert_cached_metadata_and_initial_routing(sender_core.map);
            }
            // SAFETY: all retired objects target the other persistent endpoint and transfer once.
            unsafe { sender_core.remote.post(sender_core.map, sender) };
        }
        let mut message_count = 0;
        let consume = |message| {
            // SAFETY: the target queue hands over this initialized message exactly once.
            unsafe { forwarder_owner.queue().enqueue(message, message) };
            message_count += 1;
            true
        };
        // SAFETY: this test is the target queue's sole consumer and transfers each dequeued message once.
        unsafe { target_owner.queue().drain(consume) };
        assert_eq!(message_count, 2);
        forwarder_core.drain();
        let mut budget = usize::MAX;
        assert_eq!(forwarder_core.remote.observe(forwarder_core.map, &mut budget).messages, 1);
        forwarder_core.remote.assert_cached_metadata_and_initial_routing(forwarder_core.map);
        forwarder_core.flush();
        target_core.flush();
        sender_core.flush();
    }

    #[test]
    fn gamma_round2_unarmed_hint_miss_preserves_the_observed_metadata() {
        let mut hint = LocalHint {
            base: 0,
            size: 0,
            meta: 0x4000,
        };
        assert_eq!(hint.lookup(0x8000), None);
        assert_eq!((hint.base, hint.size, hint.meta), (0, 0, 0x4000));
        hint.observe(0x8000, classes::CHUNK, 0x4000);
        assert_eq!(hint.lookup(0x8000), Some(0x4000));
    }

    #[test]
    fn gamma_round2_new_slabs_and_both_unhinted_paths_preserve_exact_class_metadata() {
        let address = fresh_owner();
        // SAFETY: The fresh persistent owner belongs exclusively to this test.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: This test exclusively leases the fresh owner's core.
        let core = unsafe { &mut *owner.core_ptr() };
        for index in [0, 27, 28, COUNT - 1] {
            let class = CLASSES[index];
            let request = Request::new(std::alloc::Layout::from_size_align(class.size, 16).unwrap()).unwrap();
            let pointers: Vec<_> = (0..3).map(|_| core.allocate(request).addr()).collect();
            assert!(pointers.iter().all(|&pointer| pointer != 0));
            // SAFETY: All three live allocations retain their immutable frontend entry.
            let meta = unsafe { core.map.lookup(pointers[0]) }.0;
            // SAFETY: This test exclusively leases the initialized, live slab.
            let slab = unsafe { &*(meta as *const Slab) };
            let base = slab.base;
            assert_eq!(usize::from(slab.tag), SMALL_TAG + index);
            assert_eq!(slab.needed, class.waking);
            assert!(slab.sleeping);
            assert_eq!((slab.prev, slab.next), (0, 0));
            for offset in (0..class.slab).step_by(classes::CHUNK) {
                assert_eq!(core.realloc_map().lookup(base + offset), Some(meta));
            }
            // SAFETY: The first live object transfers once through the local index path.
            unsafe { core.deallocate_unhinted(pointers[0]) };
            assert_eq!(
                (core.local_hint.base, core.local_hint.size, core.local_hint.meta),
                (base, class.slab, meta)
            );
            core.realloc_map().remove(base, class.slab, meta);
            core.local_hint = LocalHint { base: 0, size: 0, meta };
            // SAFETY: The second live object retains its pagemap entry despite deliberate index eviction.
            unsafe { core.deallocate_unhinted(pointers[1]) };
            assert_eq!(
                (core.local_hint.base, core.local_hint.size, core.local_hint.meta),
                (base, class.slab, meta)
            );
            let mut budget = usize::MAX;
            assert!(core.observe(&mut budget).local.requested_bytes >= class.slab as u64);
            // SAFETY: The last live allocation transfers once to the same owner.
            unsafe { core.deallocate(pointers[2]) };
            core.flush();
            core.validate(&BTreeSet::new());
            assert!(core.is_empty());
        }
    }

    #[test]
    fn gamma_round2_releasing_another_slab_preserves_hint_then_retiring_its_slab_clears_it() {
        let address = fresh_owner();
        // SAFETY: The fresh owner is persistent and belongs to this test.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: This test holds the fresh owner's only core lease.
        let core = unsafe { &mut *owner.core_ptr() };
        let small = Request::new(std::alloc::Layout::from_size_align(16, 16).unwrap()).unwrap();
        let other = Request::new(std::alloc::Layout::from_size_align(32, 16).unwrap()).unwrap();
        let retained = core.allocate(small).addr();
        let retired = core.allocate(other).addr();
        assert_ne!(retained, 0);
        assert_ne!(retired, 0);
        // SAFETY: Both objects are live and retain their immutable metadata.
        let retained_meta = unsafe { core.map.lookup(retained) }.0;
        // SAFETY: This object remains live until the unique return below.
        let retired_meta = unsafe { core.map.lookup(retired) }.0;
        // SAFETY: This is the unique owner-exclusive return of the second allocation.
        unsafe { core.deallocate(retired) };
        core.local_hint = LocalHint {
            base: retained,
            size: CLASSES[0].slab,
            meta: retained_meta,
        };
        core.flush();
        assert_eq!(
            (core.local_hint.base, core.local_hint.size, core.local_hint.meta),
            (retained, CLASSES[0].slab, retained_meta)
        );
        assert_eq!(core.realloc_map().lookup(retired), None);
        assert_eq!((core.available[1].length, core.available[1].unused), (0, 0));
        assert_ne!(retained_meta, retired_meta);
        // SAFETY: The retained allocation still belongs to this live, cached-hint slab.
        unsafe { core.deallocate(retained) };
        core.reclaim(0);
        assert_eq!((core.local_hint.base, core.local_hint.size, core.local_hint.meta), (0, 0, 0));
        assert_eq!(core.realloc_map().lookup(retained), None);
        core.validate(&BTreeSet::new());
        assert!(core.is_empty());
    }

    #[test]
    fn gamma_round2_batch_return_crosses_waking_threshold_with_one_residual_object() {
        let address = fresh_owner();
        // SAFETY: The fresh persistent owner belongs exclusively to this test.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: This test uniquely leases the fresh owner's core.
        let core = unsafe { &mut *owner.core_ptr() };
        let request = Request::new(std::alloc::Layout::from_size_align(16, 16).unwrap()).unwrap();
        let pointers: Vec<_> = (0..33).map(|_| core.allocate(request).addr()).collect();
        assert!(pointers.iter().all(|&pointer| pointer != 0));
        // SAFETY: The live objects all retain the same immutable slab publication.
        let meta = unsafe { core.map.lookup(pointers[0]) }.0;
        for pair in pointers.windows(2) {
            // SAFETY: Each exclusively owned live object has one writable aligned link word.
            unsafe { *(pair[0] as *mut usize) = pair[1] };
        }
        // SAFETY: The initialized links describe exactly 33 distinct objects returned once as a batch.
        unsafe { core.return_local(meta, pointers[0], pointers[32], 33) };
        // SAFETY: The slab remains live with 991 objects retained in the fast list.
        let slab = unsafe { &*(meta as *const Slab) };
        assert_eq!((slab.sleeping, slab.needed), (false, 991));
        assert_eq!((slab.free_head, slab.free_end), (pointers[0], pointers[32]));
        assert_eq!((core.available[0].length, core.available[0].unused), (1, 0));
        core.flush();
        core.validate(&BTreeSet::new());
        assert!(core.is_empty());
    }

    #[test]
    fn gamma_round2_single_returns_cross_the_sleeping_slab_threshold_exactly() {
        let address = fresh_owner();
        // SAFETY: fresh_owner initializes a persistent endpoint used only by this test.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: This test exclusively leases the fresh owner's core.
        let core = unsafe { &mut *owner.core_ptr() };
        let request = Request::new(std::alloc::Layout::from_size_align(16, 16).unwrap()).unwrap();
        let pointers: Vec<_> = (0..32).map(|_| core.allocate(request).addr()).collect();
        assert!(pointers.iter().all(|&pointer| pointer != 0));
        // SAFETY: These live allocations keep their published slab entry immutable.
        let meta = unsafe { core.map.lookup(pointers[0]) }.0;
        // SAFETY: Metadata is stable under this unique owner lease.
        assert_eq!(unsafe { (*(meta as *const Slab)).needed }, 32);
        for (index, &pointer) in pointers.iter().enumerate() {
            // SAFETY: Each allocated object transfers exactly once back to its live owning slab.
            unsafe { core.return_one(meta, pointer) };
            // SAFETY: The slab remains live with 992 objects retained in the core's fast list.
            let slab = unsafe { &*(meta as *const Slab) };
            assert_eq!(slab.sleeping, index != 31);
            assert_eq!(slab.needed, if index == 31 { 992 } else { u16::try_from(31 - index).unwrap() });
            assert_eq!(core.available[0].length, usize::from(index == 31));
        }
        core.flush();
        core.validate(&BTreeSet::new());
        assert!(core.is_empty());
    }

    #[test]
    fn gamma_round2_realloc_index_half_open_removal_preserves_adjacent_same_metadata() {
        let mut index = ReallocMap::new();
        let base = 4 * classes::CHUNK;
        index.insert(base, 3 * classes::CHUNK, 0x2000);
        index.remove(base, 2 * classes::CHUNK, 0x2000);
        assert_eq!(index.lookup(base), None);
        assert_eq!(index.lookup(base + classes::CHUNK), None);
        assert_eq!(index.lookup(base + 2 * classes::CHUNK), Some(0x2000));
        assert_eq!(index.lookup(base + 3 * classes::CHUNK - 1), Some(0x2000));
    }

    #[test]
    fn gamma_survivor_local_hint_boundaries_and_repeated_observation() {
        let mut hint = LocalHint::EMPTY;
        let (base, size, meta) = (0x40000, classes::CHUNK, 0x80000);
        hint.observe(base, size, meta);
        assert_eq!((hint.base, hint.size, hint.meta), (0, 0, meta));
        hint.observe(base, size, meta);
        assert_eq!(hint.lookup(base), Some(meta));
        assert_eq!(hint.lookup(base + size - 1), Some(meta));
        hint.observe(base, size, meta);
        assert_eq!((hint.base, hint.size, hint.meta), (0, 0, meta));
        assert_eq!(hint.lookup(base), None);
        hint.observe(base, size, meta);
        assert_eq!(hint.lookup(base - 1), None);
        assert_eq!((hint.base, hint.size, hint.meta), (0, 0, 0));
        hint.observe(base, size, meta);
        hint.observe(base, size, meta);
        assert_eq!(hint.lookup(base + size), None);
    }

    #[test]
    fn gamma_survivor_realloc_index_hash_collisions_update_and_removal() {
        let mut index = ReallocMap::new();
        for (chunk, expected) in [(1, 2), (31, 62), (32, 0), (127, 62), (128, 2), (255, 60), (256, 4), (4096, 0)] {
            assert_eq!(ReallocMap::set(chunk * classes::CHUNK), expected);
        }
        let first = classes::CHUNK;
        let second = 128 * classes::CHUNK;
        let third = 33 * classes::CHUNK;
        index.insert(first, classes::CHUNK, 0x1000);
        index.insert(second, classes::CHUNK, 0x2000);
        index.insert(first, classes::CHUNK, 0x3000);
        assert_eq!(index.lookup(first + classes::CHUNK - 1), Some(0x3000));
        assert_eq!(index.lookup(second), Some(0x2000));
        index.remove(first, classes::CHUNK, 0x1000);
        assert_eq!(index.lookup(first), Some(0x3000));
        index.insert(third, classes::CHUNK, 0x4000);
        assert_eq!(index.lookup(second), None);
        assert_eq!(index.lookup(third), Some(0x4000));
        index.remove(first, classes::CHUNK, 0x3000);
        assert_eq!(index.lookup(first), None);
        assert_eq!(index.lookup(third), Some(0x4000));
        index.insert(4 * classes::CHUNK, 3 * classes::CHUNK, 0x5000);
        for chunk in 4..7 {
            assert_eq!(index.lookup(chunk * classes::CHUNK), Some(0x5000));
        }
        assert_eq!(index.lookup(7 * classes::CHUNK), None);
        index.remove(4 * classes::CHUNK, 3 * classes::CHUNK, 0x5000);
        for chunk in 4..7 {
            assert_eq!(index.lookup(chunk * classes::CHUNK), None);
        }
    }

    #[test]
    fn gamma_survivor_owner_smallest_class_observation_and_reclaim() {
        let address = fresh_owner();
        // SAFETY: This test owns a fresh persistent endpoint.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: No other caller leases this test's private endpoint.
        let core = unsafe { &mut *owner.core_ptr() };
        // SAFETY: The test exclusively owns inactive pool linkage.
        assert_eq!(unsafe { owner.pool_next() }, 0);
        let request = Request::new(std::alloc::Layout::from_size_align(16, 16).unwrap()).unwrap();
        let pointers: Vec<_> = (0..5000).map(|_| core.allocate(request)).collect();
        assert!(pointers.iter().all(|pointer| !pointer.is_null()));
        let mut live: BTreeSet<_> = pointers.iter().map(|pointer| pointer.addr()).collect();
        assert_eq!(live.len(), pointers.len());
        core.validate(&live);
        let mut budget = usize::MAX;
        let observation = core.observe(&mut budget);
        assert!(observation.classes[0].observed_slabs >= 4);
        assert_eq!((observation.classes[0].object_bytes, observation.classes[0].capacity), (16, 1024));
        for (index, pointer) in pointers.into_iter().enumerate() {
            assert!(live.remove(&pointer.addr()));
            // SAFETY: This private core returns each checked live allocation exactly once.
            unsafe { core.deallocate(pointer.addr()) };
            if index % 127 == 0 {
                core.validate(&live);
            }
        }
        core.flush();
        core.validate(&live);
        assert!(core.is_empty());
        let mut budget = usize::MAX;
        let observation = core.observe(&mut budget);
        assert!(!observation.classes[0].fast_nonempty);
        assert!(observation.classes[0].empty_slabs <= 2);
        assert_eq!(core.local_hint.size, 0);
    }

    #[test]
    fn gamma_survivor_reclaim_threshold_requires_both_count_and_fraction() {
        for slabs in [2, 3, 4, 8, 12, 16] {
            let address = fresh_owner();
            // SAFETY: The fresh persistent endpoint is initialized and remains live.
            let owner = unsafe { &*(address as *const Owner) };
            // SAFETY: The test owns this fresh persistent endpoint's unique core lease.
            let core = unsafe { &mut *owner.core_ptr() };
            let request = Request::new(std::alloc::Layout::from_size_align(16, 16).unwrap()).unwrap();
            let pointers: Vec<_> = (0..slabs * 1024).map(|_| core.allocate(request).addr()).collect();
            assert!(pointers.iter().all(|&pointer| pointer != 0));
            let mut live: BTreeSet<_> = pointers.iter().copied().collect();
            for slab in pointers.chunks(1024) {
                for &pointer in &slab[..32] {
                    live.remove(&pointer);
                    // SAFETY: Transfer each live object once to its exact owner's slab.
                    unsafe { core.deallocate(pointer) };
                }
            }
            assert_eq!((core.available[0].length, core.available[0].unused), (slabs, 0));
            let (mut length, mut empty) = (slabs, 0);
            for slab in pointers.chunks(1024) {
                for &pointer in &slab[32..] {
                    assert!(live.remove(&pointer));
                    // SAFETY: No object is freed twice and this test remains the sole core owner.
                    unsafe { core.deallocate(pointer) };
                }
                empty += 1;
                if empty > 2 && empty > length / 4 {
                    length -= empty;
                    empty = 0;
                }
                assert_eq!((core.available[0].length, core.available[0].unused), (length, empty));
                core.validate(&live);
            }
            core.flush();
            assert_eq!((core.available[0].length, core.available[0].unused), (0, 0));
            assert!(core.is_empty());
        }
    }

    #[test]
    fn gamma_survivor_remote_observation_preserves_odd_class_byte_budget() {
        let target = fresh_owner();
        let sender = fresh_owner();
        // SAFETY: These fresh persistent endpoints are initialized and disjoint.
        let target_owner = unsafe { &*(target as *const Owner) };
        // SAFETY: The sender endpoint is also initialized and remains live.
        let sender_owner = unsafe { &*(sender as *const Owner) };
        // SAFETY: These two fresh persistent endpoints have disjoint, exclusively test-owned core leases.
        let target_core = unsafe { &mut *target_owner.core_ptr() };
        // SAFETY: The sender core is distinct from the target core and has no other lease holder.
        let sender_core = unsafe { &mut *sender_owner.core_ptr() };
        let request = Request::new(std::alloc::Layout::from_size_align(32, 16).unwrap()).unwrap();
        let pointers: Vec<_> = (0..4).map(|_| target_core.allocate(request)).collect();
        assert!(pointers.iter().all(|pointer| !pointer.is_null()));
        for pointer in pointers {
            // SAFETY: Each checked live target allocation is transferred once to the sender for remote return.
            unsafe { sender_core.deallocate(pointer.addr()) };
        }
        let mut budget = usize::MAX;
        let observed = sender_core.observe(&mut budget);
        assert_eq!((observed.remote.open_rings, observed.remote.open_objects), (1, 4));
        assert_eq!(observed.remote.budget_remaining, (remote::BUDGET - 128) as u64);
        // SAFETY: The sender owns all pending remote objects and posts once to the different target endpoint.
        unsafe { sender_core.remote.post(sender_core.map, sender) };
        let incoming = target_core.observe(&mut budget);
        assert_ne!(incoming.remote.incoming_front, 0);
        assert_ne!(incoming.remote.incoming_back, 0);
        assert_eq!(incoming.remote.incoming_front, incoming.remote.incoming_back);
        let pointer = target_core.allocate(request);
        assert!(!pointer.is_null());
        // SAFETY: Return the new live object once; queued remote objects retain their separate ownership.
        unsafe { target_core.deallocate(pointer.addr()) };
        target_core.flush();
    }

    #[test]
    fn gamma_survivor_slab_observation_arms_hint_in_index_and_map_paths() {
        let address = fresh_owner();
        // SAFETY: The fresh persistent endpoint is initialized and remains live.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: This fresh persistent endpoint's core is exclusively leased to the test.
        let core = unsafe { &mut *owner.core_ptr() };
        let request = Request::new(std::alloc::Layout::from_size_align(48, 16).unwrap()).unwrap();
        let pointers: Vec<_> = (0..3).map(|_| core.allocate(request).addr()).collect();
        assert!(pointers.iter().all(|&pointer| pointer != 0));
        // SAFETY: All three live allocations retain immutable slab metadata.
        let meta = unsafe { core.map.lookup(pointers[0]) }.0;
        // SAFETY: The test retains the owner's sole lease and the slab has live objects.
        let slab = unsafe { &*(meta as *const Slab) };
        let base = slab.base;
        let size = CLASSES[classes::index(48)].slab;
        assert_eq!((core.local_hint.base, core.local_hint.size, core.local_hint.meta), (0, 0, meta));
        // SAFETY: Retire the first live object once, exercising the index path rather than the hint.
        unsafe { core.deallocate_unhinted(pointers[0]) };
        assert_eq!(
            (core.local_hint.base, core.local_hint.size, core.local_hint.meta),
            (base, size, meta)
        );
        core.local_hint = LocalHint::EMPTY;
        // SAFETY: The second object remains live and is returned once through the same index path.
        unsafe { core.deallocate_unhinted(pointers[1]) };
        assert_eq!((core.local_hint.base, core.local_hint.size, core.local_hint.meta), (0, 0, meta));
        core.realloc_map().remove(base, size, meta);
        core.local_hint = LocalHint::EMPTY;
        // SAFETY: The last live object remains correctly published in the pagemap despite index eviction.
        unsafe { core.deallocate_unhinted(pointers[2]) };
        assert_eq!((core.local_hint.base, core.local_hint.size, core.local_hint.meta), (0, 0, meta));
        core.flush();
        assert!(core.is_empty());
    }

    fn fresh_owner() -> usize {
        let map = crate::backend::snmalloc::ranges::map().unwrap();
        // SAFETY: The handle comes from this backend's unique global map.
        let mut metadata = unsafe { Local::new(map) };
        let address = metadata.alloc_meta(Owner::ALLOC_SIZE);
        assert_ne!(address, 0);
        // SAFETY: Before initialization/donation, the test exclusively owns the
        // complete backing. Mark only the gap, which initialization must not touch.
        let gap = unsafe { (address as *mut u8).add(std::mem::size_of::<Owner>()) };
        // SAFETY: The still-undonated gap is within this exclusively owned backing.
        unsafe { gap.write_bytes(0x6d, Owner::LOGICAL_SIZE - std::mem::size_of::<Owner>()) };
        // SAFETY: Test owns fresh persistent metadata backing. It deliberately
        // never returns the Owner prefix, just like the production owner pool.
        unsafe { Owner::initialize(map, address) };
        address
    }

    #[test]
    fn usable_size_reads_local_and_foreign_allocations() {
        let target = fresh_owner();
        let observer = fresh_owner();
        // SAFETY: Both fresh endpoints remain persistent throughout this test.
        let target_owner = unsafe { (target as *const Owner).as_ref().unwrap() };
        // SAFETY: The test exclusively leases the target core.
        let target_core = unsafe { &mut *target_owner.core_ptr() };
        // SAFETY: The observer is a distinct fresh persistent endpoint.
        let observer_owner = unsafe { (observer as *const Owner).as_ref().unwrap() };
        // SAFETY: The test exclusively leases the distinct observer core.
        let observer_core = unsafe { &mut *observer_owner.core_ptr() };
        for (size, expected) in [(48, 48), (70_000, 131_072)] {
            let request = Request::new(std::alloc::Layout::from_size_align(size, 16).unwrap()).unwrap();
            let pointer = target_core.allocate(request);
            assert!(!pointer.is_null());
            // SAFETY: The checked allocation remains live under its sole owner's lease.
            assert_eq!(unsafe { target_core.usable_size(pointer.addr()) }, expected);
            // SAFETY: The foreign allocation remains live and is not concurrently freed.
            assert_eq!(unsafe { observer_core.usable_size(pointer.addr()) }, expected);
            // SAFETY: The lookups ended; the owner returns the live allocation exactly once.
            unsafe { target_core.deallocate(pointer.addr()) };
        }
        observer_core.flush();
        target_core.flush();
    }

    #[test]
    fn observations_classify_large_ranges_and_report_bounded_walks() {
        let address = fresh_owner();
        // SAFETY: The test owns the freshly initialized persistent endpoint.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: The test exclusively leases this endpoint's core.
        let core = unsafe { &mut *owner.core_ptr() };
        let small = core.allocate(Request::new(std::alloc::Layout::from_size_align(48, 16).unwrap()).unwrap());
        let large = core.allocate(Request::new(std::alloc::Layout::from_size_align(70_000, 16).unwrap()).unwrap());
        assert!(!small.is_null() && !large.is_null());
        let mut budget = crate::backend::snmalloc::observation::WALK_BUDGET;
        let observed = core.observe(&mut budget);
        assert!(observed.slabs_complete);
        assert!(observed.classes[classes::index(48)].observed_slabs > 0);
        assert!(observed.classes[classes::index(48)].fast_nonempty);
        assert!(!observed.classes[classes::index(32)].fast_nonempty);
        assert_eq!(observed.large.counts[17], 1, "native tags must be converted to size exponents");
        let mut one_node = 1;
        let partial = core.observe(&mut one_node);
        assert_eq!(one_node, 0);
        assert_eq!(partial.classes[classes::index(48)].observed_slabs, 0);
        assert_eq!(partial.large.counts[17], 1);
        assert!(!partial.slabs_complete);
        assert!(!partial.large.complete);
        let mut exhausted = 0;
        let partial = core.observe(&mut exhausted);
        assert!(!partial.slabs_complete);
        assert!(!partial.large.complete);
        // SAFETY: The checked small allocation remains live and is returned once.
        unsafe { core.deallocate(small.addr()) };
        // SAFETY: The checked large allocation remains live and is returned once.
        unsafe { core.deallocate(large.addr()) };
        core.flush();
    }

    #[test]
    fn observing_outgoing_returns_does_not_post_or_drain_them() {
        let target = fresh_owner();
        let sender = fresh_owner();
        // SAFETY: The target endpoint is fresh and persistent.
        let target_owner = unsafe { &*(target as *const Owner) };
        // SAFETY: This test exclusively owns the target core.
        let target_core = unsafe { &mut *target_owner.core_ptr() };
        // SAFETY: The sender endpoint is distinct, fresh and persistent.
        let sender_owner = unsafe { &*(sender as *const Owner) };
        // SAFETY: The sender is a distinct, exclusively owned core.
        let sender_core = unsafe { &mut *sender_owner.core_ptr() };
        let pointer = target_core.allocate(Request::new(std::alloc::Layout::from_size_align(32, 16).unwrap()).unwrap());
        assert!(!pointer.is_null());
        // SAFETY: The allocation is transferred once for remote destruction.
        unsafe { sender_core.deallocate(pointer.addr()) };
        let mut budget = crate::backend::snmalloc::observation::WALK_BUDGET;
        let observed = sender_core.observe(&mut budget);
        assert_eq!(observed.remote.open_objects, 1);
        assert_eq!(observed.remote.messages, 0);
        let mut second_budget = crate::backend::snmalloc::observation::WALK_BUDGET;
        assert_eq!(sender_core.observe(&mut second_budget).remote, observed.remote);
        sender_core.flush();
        target_core.flush();
    }

    #[test]
    fn local_returns_count_every_object_toward_the_drain_budget() {
        let target = fresh_owner();
        let sender = fresh_owner();
        // SAFETY: Each fresh endpoint remains persistent for this regression.
        let target_owner = unsafe { &*(target as *const Owner) };
        // SAFETY: The test holds this fresh core's sole lease.
        let target_core = unsafe { &mut *target_owner.core_ptr() };
        // SAFETY: This is a distinct fresh persistent endpoint.
        let sender_owner = unsafe { &*(sender as *const Owner) };
        // SAFETY: The test holds this fresh core's sole lease.
        let sender_core = unsafe { &mut *sender_owner.core_ptr() };
        let request = Request::new(std::alloc::Layout::from_size_align(65_536, 16).unwrap()).unwrap();
        let mut pointers = Vec::new();
        for _ in 0..24 {
            let pointer = target_core.allocate(request);
            assert!(!pointer.is_null());
            pointers.push(pointer);
        }
        for ring in pointers.chunks_exact(4) {
            for pointer in ring {
                // SAFETY: These live allocations retain their stable owner-exclusive slabs.
                let (meta, _) = unsafe { target_core.map.lookup(pointer.addr()) };
                // SAFETY: Each allocation transfers once to the outgoing cache.
                unsafe { sender_core.remote.deallocate(sender_core.map, meta, pointer.addr()) };
            }
            // Bypass the sender's smaller byte budget to publish four-object rings.
            // SAFETY: All cached objects are exclusively held and transfer once.
            unsafe { sender_core.remote.post(sender_core.map, sender) };
        }
        target_core.drain();
        assert_eq!(target_core.outstanding_objects(), 8);
        sender_core.flush();
        target_core.flush();
    }

    #[test]
    fn forwarded_returns_leave_work_queued_at_the_drain_budget() {
        let target = fresh_owner();
        let sender = fresh_owner();
        let relay = fresh_owner();
        // SAFETY: Each fresh endpoint remains persistent for this regression.
        let target_owner = unsafe { &*(target as *const Owner) };
        // SAFETY: The test holds this fresh core's sole lease.
        let target_core = unsafe { &mut *target_owner.core_ptr() };
        // SAFETY: This is a distinct fresh persistent endpoint.
        let sender_owner = unsafe { &*(sender as *const Owner) };
        // SAFETY: The test holds this fresh core's sole lease.
        let sender_core = unsafe { &mut *sender_owner.core_ptr() };
        // SAFETY: This is a distinct fresh persistent endpoint.
        let relay_owner = unsafe { &*(relay as *const Owner) };
        // SAFETY: The test holds this fresh core's sole lease.
        let relay_core = unsafe { &mut *relay_owner.core_ptr() };
        let request = Request::new(std::alloc::Layout::from_size_align(remote::DRAIN_LIMIT / 2, 16).unwrap()).unwrap();
        let mut pointers = Vec::new();
        for _ in 0..6 {
            let pointer = target_core.allocate(request);
            assert!(!pointer.is_null());
            pointers.push(pointer);
        }
        for pointer in pointers {
            // SAFETY: The live allocation is transferred once for remote destruction.
            unsafe { sender_core.deallocate(pointer.addr()) };
        }
        sender_core.flush();
        let mut messages = Vec::new();
        // SAFETY: The test is the sole consumer; relinquished messages retain their slabs.
        unsafe {
            target_owner.queue().drain(|message| {
                messages.push(message);
                true
            });
        }
        assert_eq!(messages.len(), 5);
        // SAFETY: The initialized segment is no longer queued and is exclusively owned.
        unsafe { relay_owner.queue().enqueue(messages[0], messages[4]) };
        relay_core.drain();
        assert_eq!(relay_owner.queue().retained_message(), Some(messages[2]));
        relay_core.drain();
        assert_eq!(relay_owner.queue().retained_message(), Some(messages[4]));
        relay_core.flush();
        target_core.flush();
        target_core.flush();
        assert_eq!(target_core.outstanding_objects(), 2);
    }

    #[test]
    fn misrouted_returns_are_forwarded_with_and_without_post_budget() {
        for exhausted in [false, true] {
            let target = fresh_owner();
            let sender = fresh_owner();
            let relay = fresh_owner();
            // SAFETY: Each endpoint is fresh and remains persistent throughout the test.
            let target_owner = unsafe { &*(target as *const Owner) };
            // SAFETY: The test is the target core's sole lease holder.
            let target_core = unsafe { &mut *target_owner.core_ptr() };
            // SAFETY: The sender is a distinct fresh persistent endpoint.
            let sender_owner = unsafe { &*(sender as *const Owner) };
            // SAFETY: The test is the sender core's sole lease holder.
            let sender_core = unsafe { &mut *sender_owner.core_ptr() };
            // SAFETY: The relay is a distinct fresh persistent endpoint.
            let relay_owner = unsafe { &*(relay as *const Owner) };
            // SAFETY: The test is the relay core's sole lease holder.
            let relay_core = unsafe { &mut *relay_owner.core_ptr() };
            for size in [32, 48, 80, 112] {
                let request = Request::new(std::alloc::Layout::from_size_align(size, 16).unwrap()).unwrap();
                let pointer = target_core.allocate(request);
                assert!(!pointer.is_null());
                // SAFETY: This live target allocation is transferred once to the sender.
                unsafe { sender_core.deallocate(pointer.addr()) };
            }
            sender_core.flush();
            let mut messages = Vec::new();
            // SAFETY: This test is the target queue's only consumer. Its retained
            // tail stays queued; relinquished messages keep their original slabs live.
            unsafe {
                target_owner.queue().drain(|message| {
                    messages.push(message);
                    true
                });
            }
            assert_eq!(messages.len(), 3);
            // Simulate the mixed-destination segment produced by a radix-bucket
            // collision: all three initialized messages now belong to the relay.
            // SAFETY: The dequeued segment is exclusively held and not queued elsewhere.
            unsafe { relay_owner.queue().enqueue(messages[0], messages[2]) };
            if exhausted {
                relay_core.remote.clear_budget();
            }
            relay_core.drain();
            assert_eq!(relay_owner.queue().retained_message(), Some(messages[2]));
            let mut budget = crate::backend::snmalloc::observation::WALK_BUDGET;
            let observed = relay_core.remote.observe(relay_core.map, &mut budget);
            assert_eq!(observed.messages, if exhausted { 0 } else { 2 });
            assert_eq!(observed.message_objects, if exhausted { 0 } else { 2 });
            if !exhausted {
                let mut one_message = 1;
                let partial = relay_core.remote.observe(relay_core.map, &mut one_message);
                assert_eq!(one_message, 0);
                assert_eq!(partial.messages, 1);
                assert_eq!(partial.message_objects, 1);
                assert!(!partial.complete);
                let mut no_messages = 0;
                let partial = relay_core.remote.observe(relay_core.map, &mut no_messages);
                assert_eq!(partial.messages, 0);
                assert!(!partial.complete);
            }
            relay_core.flush();
            target_core.flush();
            // One message remains retained by each queue until another
            // publisher links a successor; forwarding must return the other two.
            assert_eq!(target_core.outstanding_objects(), 2);
        }
    }

    #[test]
    fn local_hint_arms_after_repeated_slab_observation_and_clears_on_miss() {
        let mut hint = LocalHint::EMPTY;
        let base = 0x40000;
        let size = classes::CHUNK;
        let meta = 0x80000;
        hint.observe(base, size, meta);
        assert_eq!(hint.lookup(base), None);
        hint.observe(base, size, meta);
        assert_eq!(hint.lookup(base + 32), Some(meta));
        assert_eq!(hint.lookup(base + size), None);
        assert_eq!(hint.lookup(base + 32), None);
    }

    #[test]
    fn large_realloc_index_retains_only_the_object_base() {
        let address = fresh_owner();
        // SAFETY: This fresh endpoint remains persistent throughout the test.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: The test retains the core's sole lease.
        let core = unsafe { &mut *owner.core_ptr() };
        let size = 8 << 20;
        let request = Request::new(std::alloc::Layout::from_size_align(size, 16).unwrap()).unwrap();
        let pointer = core.allocate(request);
        assert!(!pointer.is_null());
        let base = pointer.addr();
        // SAFETY: Both addresses lie in the checked live range.
        let (meta, encoded) = unsafe { core.map.lookup(base) };
        assert_eq!(encoded, address | request.tag());
        assert_eq!(core.realloc_map().lookup(base), Some(meta));
        assert_eq!(core.realloc_map().lookup(base + classes::CHUNK), None);
        // SAFETY: The entire large range must retain its frontend routing entry.
        assert_eq!(unsafe { core.map.lookup(base + size - 1) }, (meta, encoded));
        // SAFETY: The unique live allocation is retired once.
        unsafe { core.deallocate(base) };
        assert_eq!(core.realloc_map().lookup(base), None);
        assert!(core.is_empty());
    }

    #[test]
    fn realloc_map_eviction_and_removal_preserve_other_way() {
        let mut map = ReallocMap::new();
        let first = classes::CHUNK;
        let set = ReallocMap::set(first);
        let mut colliding = (2..=256)
            .map(|index| index * classes::CHUNK)
            .filter(|&chunk| ReallocMap::set(chunk) == set);
        let second = colliding.next().unwrap();
        let third = colliding.next().unwrap();

        map.insert(first, classes::CHUNK, 0x1000);
        map.insert(second, classes::CHUNK, 0x2000);
        assert_eq!(map.lookup(first), Some(0x1000));
        assert_eq!(map.lookup(second), Some(0x2000));

        map.insert(third, classes::CHUNK, 0x3000);
        assert_eq!(map.lookup(first), Some(0x1000));
        assert_eq!(map.lookup(second), None);
        assert_eq!(map.lookup(third), Some(0x3000));

        map.remove(first, classes::CHUNK, 0x1000);
        assert_eq!(map.lookup(first), None);
        assert_eq!(map.lookup(third), Some(0x3000));
    }

    #[test]
    fn map_accessor_cannot_redirect_frontend_from_backend() {
        let address = fresh_owner();
        // SAFETY: This test exclusively owns the freshly initialized core.
        let core = unsafe { (*(address as *const Owner)).core_ptr() };
        // SAFETY: This test holds the endpoint's unique lease.
        let core = unsafe { &mut *core };
        let original = core.map();
        let mut handle = core.map();
        assert_eq!(handle, original);
        handle = Map::reserve().unwrap();
        assert_ne!(handle, core.map());
        for size in [16, 48, 65536, 131_072] {
            let request = Request::new(std::alloc::Layout::from_size_align(size, 16).unwrap()).unwrap();
            let ptr = core.allocate(request);
            assert!(!ptr.is_null());
            assert_eq!(core.map(), original);
            // SAFETY: The checked allocation retains its immutable frontend entry.
            assert_eq!(unsafe { original.lookup(ptr as usize) }.1, address | request.tag());
            // SAFETY: The allocation is still live and returned exactly once.
            unsafe { core.deallocate(ptr as usize) };
        }
        core.flush();
        assert!(core.is_empty());
    }

    #[test]
    fn valid_size_realloc_failure_preserves_the_original_allocation() {
        let address = fresh_owner();
        // SAFETY: The fresh endpoint remains persistent throughout this test.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: The test is this endpoint's sole core lease.
        let core = unsafe { &mut *owner.core_ptr() };
        let original = Request::new(std::alloc::Layout::from_size_align(32, 16).unwrap()).unwrap();
        let replacement = Request::new(std::alloc::Layout::from_size_align(8 << 20, 16).unwrap()).unwrap();
        let pointer = core.allocate(original);
        assert!(!pointer.is_null());
        // SAFETY: The checked allocation exclusively owns these 32 payload bytes.
        unsafe { pointer.write_bytes(73, 32) };
        crate::backend::snmalloc::hal::fail_next(crate::backend::snmalloc::hal::Failure::Commit);
        // SAFETY: The original remains live and the copy length fits both layouts.
        assert!(unsafe { core.reallocate(pointer.addr(), replacement, 32) }.is_null());
        // SAFETY: A failed realloc must retain the original allocation and payload.
        assert!(unsafe { std::slice::from_raw_parts(pointer, 32) }.iter().all(|&byte| byte == 73));
        // SAFETY: The original allocation is still live and is retired exactly once.
        unsafe { core.deallocate(pointer.addr()) };
        core.flush();
        assert!(core.is_empty());
    }

    #[test]
    fn slab_metadata_commit_failure_returns_null_and_can_be_retried() {
        let address = fresh_owner();
        // SAFETY: This fresh endpoint's core has no live allocations or other lease.
        let core = unsafe { (*(address as *const Owner)).core_ptr() };
        // SAFETY: The test retains the endpoint's sole lease throughout this regression.
        let core = unsafe { &mut *core };
        // SAFETY: The replacement uses the same unique backend map, with no frontend entries.
        core.backend = unsafe { Local::new(core.map()) };
        let request = Request::new(std::alloc::Layout::from_size_align(16, 16).unwrap()).unwrap();
        crate::backend::snmalloc::hal::fail_next(crate::backend::snmalloc::hal::Failure::Commit);
        assert!(core.allocate(request).is_null());
        assert!(core.is_empty());
        let ptr = core.allocate(request);
        assert!(!ptr.is_null());
        // SAFETY: The successful retry's live allocation is returned once.
        unsafe { core.deallocate(ptr as usize) };
        core.flush();
        assert!(core.is_empty());
    }

    #[test]
    fn owner_spare_tail_supplies_reusable_slab_metadata() {
        let address = fresh_owner();
        let start = address + Owner::LOGICAL_SIZE;
        let end = address + Owner::ALLOC_SIZE;
        let count = (end - start) / Slab::ALLOC_SIZE;
        assert_eq!((Owner::LOGICAL_SIZE, Owner::ALLOC_SIZE, Owner::ALLOC_BITS), (9216, 16384, 14));
        assert_eq!(address % Owner::ALLOC_SIZE, 0);
        assert_eq!((Slab::ALLOC_SIZE, count), (64, 112));
        // SAFETY: The fresh owner and all metadata leased from its private
        // backend are exclusively owned by this test.
        let core = unsafe { (*(address as *const Owner)).core_ptr() };
        // SAFETY: The test holds the sole core lease.
        let core = unsafe { &mut *core };
        let original_map = core.map();
        let chunks = [(9216, 1024), (10240, 2048), (12288, 4096)];
        for (offset, size) in chunks {
            assert_eq!(core.backend.alloc_meta(size), address + offset);
        }
        for (offset, size) in chunks.into_iter().rev() {
            // SAFETY: Each exact allocated metadata block is returned once.
            unsafe { core.backend.free_meta(address + offset, size) };
        }
        for _ in 0..4 {
            let mut blocks = Vec::new();
            for _ in 0..count {
                let block = core.backend.alloc_meta(Slab::ALLOC_SIZE);
                assert!(block >= start && block + Slab::ALLOC_SIZE <= end);
                // SAFETY: The checked metadata cell is committed and exclusively writable.
                unsafe { (block as *mut u8).write_bytes(0xa5, Slab::ALLOC_SIZE) };
                blocks.push(block);
            }
            assert_eq!(blocks.iter().copied().collect::<std::collections::BTreeSet<_>>().len(), count);
            let overflow = core.backend.alloc_meta(Slab::ALLOC_SIZE);
            assert_ne!(overflow, 0);
            assert!(overflow + Slab::ALLOC_SIZE <= address || overflow >= end);
            // SAFETY: The extra metadata allocation is retired once.
            unsafe { core.backend.free_meta(overflow, Slab::ALLOC_SIZE) };
            for block in blocks.into_iter().rev() {
                // SAFETY: Each live metadata cell is returned exactly once.
                unsafe { core.backend.free_meta(block, Slab::ALLOC_SIZE) };
            }
            assert_eq!(core.map(), original_map);
            // Read only the unused withheld gap, not the owner, sidecar or donated suffix.
            let actual_end = std::mem::size_of::<Owner>();
            // SAFETY: fresh_owner initialized this disjoint, permanently unused gap.
            let gap = unsafe { std::slice::from_raw_parts((address + actual_end) as *const u8, Owner::REALLOC_MAP_OFFSET - actual_end) };
            assert!(gap.iter().all(|&byte| byte == 0x6d));
        }
        let ptr = core.allocate(Request::new(std::alloc::Layout::from_size_align(16, 16).unwrap()).unwrap());
        assert!(!ptr.is_null());
        // SAFETY: The live allocation retains its immutable map entry.
        let (metadata, _) = unsafe { original_map.lookup(ptr as usize) };
        assert!(metadata >= start && metadata + Slab::ALLOC_SIZE <= end);
        // SAFETY: The checked allocation has 16 writable bytes.
        unsafe { ptr.write_bytes(0x3c, 16) };
        // SAFETY: Offset15 lies within that allocation.
        let last = unsafe { ptr.add(15) };
        // SAFETY: That byte was initialized above.
        assert_eq!(unsafe { *last }, 0x3c);
        // SAFETY: The allocation is retired exactly once.
        unsafe { core.deallocate(ptr as usize) };
        core.flush();
        assert!(core.is_empty());
    }

    #[test]
    fn fresh_slab_chain_preserves_ascending_complete_objects() {
        let address = fresh_owner();
        // SAFETY: This fresh endpoint is exclusively leased to the test.
        let core = unsafe { (*(address as *const Owner)).core_ptr() };
        // SAFETY: The test holds the endpoint's sole core lease.
        let core = unsafe { &mut *core };
        for (index, class) in CLASSES.into_iter().enumerate() {
            let request = Request::new(std::alloc::Layout::from_size_align(class.size, 16).unwrap()).unwrap();
            let allocations = (0..usize::from(class.capacity))
                .map(|_| core.allocate(request) as usize)
                .collect::<Vec<_>>();
            let base = allocations[0];
            assert_ne!(base, 0);
            assert_eq!(base % class.slab, 0);
            assert_eq!(
                allocations,
                (0..usize::from(class.capacity))
                    .map(|slot| base + slot * class.size)
                    .collect::<Vec<_>>()
            );
            assert_eq!(core.fast[index], 0);
            let next = core.allocate(request) as usize;
            assert_ne!(next, 0);
            assert!(next < base || next >= base + class.slab);
            for pointer in allocations.into_iter().chain([next]) {
                // SAFETY: The asserted fresh chain supplies disjoint live
                // complete objects, each writable for the selected class size.
                unsafe { (pointer as *mut u8).write_bytes(0xa5, class.size) };
                // SAFETY: Each live object is returned once to its owner.
                unsafe { core.deallocate(pointer) };
            }
            core.flush();
            assert!(core.is_empty());
        }
    }

    #[test]
    fn single_return_matches_batch_order_and_threshold_transitions() {
        let left = fresh_owner();
        let right = fresh_owner();
        // SAFETY: Both endpoints are exclusively leased here. Each live object
        // is returned exactly once, except those explicitly reallocated below.
        let single = unsafe { (*(left as *const Owner)).core_ptr() };
        // SAFETY: The left endpoint is exclusively leased to this test.
        let single = unsafe { &mut *single };
        // SAFETY: The right endpoint is distinct and freshly initialized.
        let batch = unsafe { (*(right as *const Owner)).core_ptr() };
        // SAFETY: The right endpoint also has only this test's lease.
        let batch = unsafe { &mut *batch };
        for (index, class) in CLASSES.into_iter().enumerate() {
            let request = Request::new(std::alloc::Layout::from_size_align(class.size, 16).unwrap()).unwrap();
            let capacity = usize::from(class.capacity);
            let a = (0..capacity * 4).map(|_| single.allocate(request) as usize).collect::<Vec<_>>();
            let b = (0..capacity * 4).map(|_| batch.allocate(request) as usize).collect::<Vec<_>>();
            assert!(a.iter().chain(&b).all(|&ptr| ptr != 0));
            for slab in 0..4 {
                for slot in (0..capacity / 2).rev() {
                    let i = slab * capacity + slot;
                    // SAFETY: Both indexed allocations are live.
                    let (ma, _) = unsafe { single.map.lookup(a[i]) };
                    // SAFETY: The corresponding batch allocation is also live.
                    let (mb, _) = unsafe { batch.map.lookup(b[i]) };
                    // SAFETY: This exact local object is returned once.
                    unsafe { single.return_one(ma, a[i]) };
                    // SAFETY: The corresponding one-object segment is returned once.
                    unsafe { batch.return_local(mb, b[i], b[i], 1) };
                    // SAFETY: Partial returns cannot reclaim this slab.
                    let sa = unsafe { &*(ma as *const Slab) };
                    // SAFETY: The paired slab likewise retains outstanding objects.
                    let sb = unsafe { &*(mb as *const Slab) };
                    assert_eq!((sa.needed, sa.sleeping), (sb.needed, sb.sleeping));
                }
            }
            // No slab is empty: refill must return the same original
            // allocation indices, proving both slab and free-queue order.
            for _ in 0..(capacity / 2) * 4 {
                let pa = single.allocate(request) as usize;
                let pb = batch.allocate(request) as usize;
                assert_eq!(a.iter().position(|&p| p == pa).unwrap(), b.iter().position(|&p| p == pb).unwrap());
            }
            for (&pa, &pb) in a.iter().zip(&b) {
                // SAFETY: Reallocation restored every original object to live ownership.
                unsafe { single.deallocate(pa) };
                // SAFETY: The paired allocation is still live.
                let (meta, _) = unsafe { batch.map.lookup(pb) };
                // SAFETY: This exact one-object segment is retired once.
                unsafe { batch.return_local(meta, pb, pb, 1) };
                assert_eq!(
                    (
                        single.available[index].length,
                        single.available[index].unused,
                        single.outstanding_objects()
                    ),
                    (
                        batch.available[index].length,
                        batch.available[index].unused,
                        batch.outstanding_objects()
                    ),
                );
            }
            single.flush();
            batch.flush();
            assert!(single.is_empty() && batch.is_empty());
        }
    }

    fn random(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn randomized_owner_transitions_preserve_structural_invariants() {
        const SIZES: [usize; 20] = [
            1, 15, 16, 17, 31, 48, 63, 64, 65, 96, 129, 257, 513, 1025, 4097, 8193, 16_385, 65_536, 65_537, 131_073,
        ];
        for initial_seed in 1..=24 {
            let owner = fresh_owner();
            // SAFETY: This fresh endpoint is exclusively leased to the test.
            let core = unsafe { (*(owner as *const Owner)).core_ptr() };
            // SAFETY: No other thread can access this core during the test.
            let core = unsafe { &mut *core };
            let mut seed = 0x9e37_79b9_7f4a_7c15 ^ initial_seed;
            let mut live = Vec::with_capacity(256);
            let mut live_set = BTreeSet::new();
            for operation in 0..10_000 {
                let choice = random(&mut seed) % 100;
                if live.is_empty() || live.len() < 256 && choice < 47 {
                    let size = SIZES[usize::try_from(random(&mut seed)).unwrap() % SIZES.len()];
                    let alignment = 1usize << (usize::try_from(random(&mut seed)).unwrap() % 13);
                    let request = Request::new(std::alloc::Layout::from_size_align(size, alignment).unwrap()).unwrap();
                    let pointer = core.allocate(request) as usize;
                    assert_ne!(pointer, 0);
                    assert!(live_set.insert(pointer));
                    live.push(pointer);
                } else {
                    let index = usize::try_from(random(&mut seed)).unwrap() % live.len();
                    let pointer = live[index];
                    if choice < 72 {
                        let size = SIZES[usize::try_from(random(&mut seed)).unwrap() % SIZES.len()];
                        let alignment = 1usize << (usize::try_from(random(&mut seed)).unwrap() % 13);
                        let request = Request::new(std::alloc::Layout::from_size_align(size, alignment).unwrap()).unwrap();
                        // SAFETY: The selected pointer is live and zero bytes
                        // are copied, so the replacement request alone bounds access.
                        let replacement = unsafe { core.reallocate(pointer, request, 0) } as usize;
                        assert_ne!(replacement, 0);
                        assert!(live_set.remove(&pointer));
                        assert!(live_set.insert(replacement));
                        live[index] = replacement;
                    } else {
                        assert!(live_set.remove(&pointer));
                        live.swap_remove(index);
                        // SAFETY: The removed pointer was live and is retired once.
                        unsafe { core.deallocate(pointer) };
                    }
                }
                if operation % 97 == 0 {
                    core.validate(&live_set);
                }
            }
            core.validate(&live_set);
            for pointer in live {
                assert!(live_set.remove(&pointer));
                // SAFETY: Every remaining pointer is live and returned once.
                unsafe { core.deallocate(pointer) };
            }
            core.flush();
            core.validate(&live_set);
            assert!(core.is_empty());
        }
    }
}
