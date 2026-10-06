// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Owner-exclusive frontend state. The endpoint's queue is outside the
//! `UnsafeCell`, so taking the core lease does not alias concurrent publishers.

use std::cell::UnsafeCell;
use std::sync::atomic::{Ordering, fence};

use crate::backend::Local;
use crate::classes::{self, CLASSES, COUNT, Request, SMALL_TAG};
use crate::pagemap::{BACKEND, Map};
use crate::remote::{self, Cache, Queue};

#[repr(C, align(256))]
pub(crate) struct Owner {
    queue: Queue,
    state: UnsafeCell<Core>,
    pool_next: UnsafeCell<usize>,
    observation: crate::observation::Metadata,
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
    /// map must be `backend::map()`'s handle, because both global refills and this
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
                observation: crate::observation::Metadata::new(),
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

    pub(crate) fn observation(&self) -> &crate::observation::Metadata {
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
        for chunk in (base..base + size).step_by(classes::CHUNK) {
            let set = Self::set(chunk);
            let first = self.entries[set];
            let slot = if first.chunk == 0 || first.chunk == chunk { set } else { set + 1 };
            self.entries[slot] = ReallocEntry { chunk, meta };
        }
    }

    fn remove(&mut self, base: usize, size: usize, meta: usize) {
        for chunk in (base..base + size).step_by(classes::CHUNK) {
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
                crate::hal::prefetch(self.fast[index]);
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
        if !ptr.is_null() {
            // Every object subsequently popped from this prepared fast list,
            // including the directly returned first object, is sequenced after
            // the slab, pagemap, and intrusive free-object initialization.
            // Pair with pointer consumers that acquire ownership after a
            // relaxed atomic handoff.
            fence(Ordering::Release);
        }
        ptr
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
        // SAFETY: Independent metadata and object ranges are exclusively owned.
        // The stable slab is initialized before its pagemap is published.
        unsafe {
            (meta as *mut Slab).write(Slab {
                prev: 0,
                next: 0,
                free_head: 0,
                free_end: 0,
                base,
                needed: if tag >= SMALL_TAG { CLASSES[tag - SMALL_TAG].waking } else { 1 },
                tag: encoded_tag,
                sleeping: tag >= SMALL_TAG,
            });
        }
        // SAFETY: The initialized slab and registered object range share this owner.
        unsafe { self.map.assign(base, size, meta, self.owner | tag) };
        self.local_hint.observe(base, size, meta);
        self.realloc_map().insert(base, size, meta);
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
        crate::hal::prefetch(self.fast[index]);
        first as *mut u8
    }

    /// # Safety
    /// ptr is the start of an exclusively owned live allocation in this map.
    pub(crate) unsafe fn deallocate(&mut self, ptr: usize) {
        if let Some(meta) = self.local_hint.lookup(ptr) {
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
        if let Some(meta) = self.realloc_map().lookup(ptr) {
            // SAFETY: The local index retains this initialized local slab.
            let slab = unsafe { &*(meta as *const Slab) };
            let tag = usize::from(slab.tag);
            let size = if tag >= SMALL_TAG {
                CLASSES[tag - SMALL_TAG].slab
            } else {
                classes::size(tag)
            };
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
        fence(Ordering::Acquire);
        // SAFETY: A live allocation guarantees its immutable frontend entry.
        let (meta, encoded) = unsafe { self.map.lookup(ptr) };
        if encoded & !255 == self.owner {
            // SAFETY: The current pagemap snapshot identifies this live local slab.
            let slab = unsafe { &*(meta as *const Slab) };
            let size = if usize::from(slab.tag) >= SMALL_TAG {
                CLASSES[usize::from(slab.tag) - SMALL_TAG].slab
            } else {
                classes::size(usize::from(slab.tag))
            };
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
        let local = self.realloc_map().lookup(ptr);
        if local.is_none() {
            // Pair with a relaxed atomic pointer handoff before reading or
            // routing a replacement for a non-local allocation.
            fence(Ordering::Acquire);
        }
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
            if slab.sleeping {
                slab.sleeping = false;
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
                break;
            }
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
        let size = if tag >= SMALL_TAG {
            CLASSES[tag - SMALL_TAG].slab
        } else {
            classes::size(tag)
        };
        if self.local_hint.meta == meta {
            self.local_hint = LocalHint::EMPTY;
        }
        self.realloc_map().remove(base, size, meta);
        // SAFETY: No frontend reader remains for these registered entries.
        unsafe { self.map.assign(base, size, 0, BACKEND) };
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
        let mut bytes = 0usize;
        let mut post = false;
        let consume = |message| {
            // SAFETY: Queue drain hands over a live message and its retained slab.
            let (meta, encoded) = unsafe { self.map.lookup(message) };
            // SAFETY: The dequeued message's initialized ring is exclusively owned.
            let (first, count) = unsafe { remote::open(message) };
            let amount = classes::size(encoded & 127) * usize::from(count);
            if encoded & !255 == self.owner {
                bytes += amount;
                // SAFETY: The owner matches and the exact ring transfers once.
                unsafe { self.return_local(meta, first, message, count) };
            } else {
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
        while queue.can_dequeue() {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn fresh_owner() -> usize {
        let map = crate::backend::map().unwrap();
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
    fn observations_classify_large_ranges_and_report_bounded_walks() {
        let address = fresh_owner();
        // SAFETY: The test owns the freshly initialized persistent endpoint.
        let owner = unsafe { &*(address as *const Owner) };
        // SAFETY: The test exclusively leases this endpoint's core.
        let core = unsafe { &mut *owner.core_ptr() };
        let small = core.allocate(Request::new(std::alloc::Layout::from_size_align(48, 16).unwrap()).unwrap());
        let large = core.allocate(Request::new(std::alloc::Layout::from_size_align(70_000, 16).unwrap()).unwrap());
        assert!(!small.is_null() && !large.is_null());
        let mut budget = crate::observation::WALK_BUDGET;
        let observed = core.observe(&mut budget);
        assert!(observed.slabs_complete);
        assert!(observed.classes[classes::index(48)].observed_slabs > 0);
        assert_eq!(observed.large.counts[17], 1, "native tags must be converted to size exponents");
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
        let mut budget = crate::observation::WALK_BUDGET;
        let observed = sender_core.observe(&mut budget);
        assert_eq!(observed.remote.open_objects, 1);
        assert_eq!(observed.remote.messages, 0);
        let mut second_budget = crate::observation::WALK_BUDGET;
        assert_eq!(sender_core.observe(&mut second_budget).remote, observed.remote);
        sender_core.flush();
        target_core.flush();
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
    fn realloc_map_eviction_and_removal_preserve_other_way() {
        let mut map = ReallocMap::new();
        let first = classes::CHUNK;
        let set = ReallocMap::set(first);
        let mut colliding = (2..)
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
    fn slab_metadata_commit_failure_returns_null_and_can_be_retried() {
        let address = fresh_owner();
        // SAFETY: This fresh endpoint's core has no live allocations or other lease.
        let core = unsafe { (*(address as *const Owner)).core_ptr() };
        // SAFETY: The test retains the endpoint's sole lease throughout this regression.
        let core = unsafe { &mut *core };
        // SAFETY: The replacement uses the same unique backend map, with no frontend entries.
        core.backend = unsafe { Local::new(core.map()) };
        let request = Request::new(std::alloc::Layout::from_size_align(16, 16).unwrap()).unwrap();
        crate::hal::fail_next(crate::hal::Failure::Commit);
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
