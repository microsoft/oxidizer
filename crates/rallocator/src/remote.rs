// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `BatchedRemoteMessage`, `RemoteDeallocCache` and the tail-retaining MPSC queue.
//! A message occupies exactly two words in an already freed object. The first
//! word packs a signed same-slab displacement with an 11-bit ring length.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::core::Owner;
use crate::pagemap::Map;

const LENGTH_BITS: usize = 11;
const LENGTH_MASK: usize = (1 << LENGTH_BITS) - 1;
pub(crate) const BUDGET: usize = 16 * 1024;
pub(crate) const DRAIN_LIMIT: usize = 1024 * 1024;

#[repr(C)]
struct Message {
    ring: usize,
    next: AtomicUsize,
}

/// # Safety
/// `message` is an initialized `Message` and remains live, without changing its
/// storage type, for the entire lifetime of the returned reference.
unsafe fn next<'a>(message: usize) -> &'a AtomicUsize {
    // SAFETY: The second word has been initialized as an AtomicUsize. Queue
    // lifetime discipline keeps it allocated until its publisher has linked.
    unsafe { &(*(message as *const Message)).next }
}

/// # Safety
/// The message's ring word is initialized and not being modified.
pub(crate) unsafe fn open(message: usize) -> (usize, u16) {
    // SAFETY: The first word is immutable while a message is queued.
    let packed = unsafe { (*(message as *const Message)).ring };
    #[expect(
        clippy::cast_possible_wrap,
        reason = "The high bits encode a two's-complement signed same-slab displacement"
    )]
    let signed = packed as isize;
    #[expect(
        clippy::cast_sign_loss,
        reason = "Wrapping address addition reconstructs negative as well as positive displacements"
    )]
    let offset = (signed >> LENGTH_BITS) as usize;
    #[expect(clippy::cast_possible_truncation, reason = "The mask retains only the 11-bit ring length")]
    let count = (packed & LENGTH_MASK) as u16;
    (message.wrapping_add(offset), count)
}

#[repr(align(64))]
struct CacheLine(AtomicUsize);

pub(crate) struct Queue {
    back: CacheLine,
    front: CacheLine,
}

impl Queue {
    pub(crate) fn observe(&self) -> (u64, u64) {
        // Producer exchanges may be in flight. These are independent atomic
        // observations, not a queue-length census or permission to dereference.
        (
            self.front.0.load(Ordering::Acquire) as u64,
            self.back.0.load(Ordering::Acquire) as u64,
        )
    }

    pub(crate) const fn new() -> Self {
        Self {
            back: CacheLine(AtomicUsize::new(0)),
            front: CacheLine(AtomicUsize::new(0)),
        }
    }

    // #[gamma::skip(fn_value.bool_true, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    pub(crate) fn can_dequeue(&self) -> bool {
        // #[gamma::skip(bool_expr.negate, relational.ne_to_eq, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        self.front.0.load(Ordering::Relaxed) != self.back.0.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn retained_message(&self) -> Option<usize> {
        let head = self.front.0.load(Ordering::Acquire);
        (head != 0).then_some(head)
    }

    /// # Safety
    /// This is a permanently located queue with exactly one consumer. Segment
    /// messages are initialized, linked, exclusively held and not already queued.
    pub(crate) unsafe fn enqueue(&self, first: usize, last: usize) {
        // SAFETY: Segment exclusively belongs to the publishing thread until
        // the exchange; the old tail cannot be reclaimed before we link it.
        unsafe { next(last) }.store(0, Ordering::Relaxed);
        let previous = self.back.0.swap(last, Ordering::AcqRel);
        if previous != 0 {
            // SAFETY: The retained previous tail stays live until this link publishes.
            unsafe { next(previous) }.store(first, Ordering::Release);
        } else {
            self.front.0.store(first, Ordering::Release);
        }
    }

    /// # Safety
    /// The caller is the only consumer. Callback accepts ownership of each
    /// dequeued message and must not recursively consume this queue.
    pub(crate) unsafe fn drain(&self, mut consume: impl FnMut(usize) -> bool) {
        let Some(mut current) = initial_drain_front(self.front.0.load(Ordering::Acquire)) else {
            return;
        };
        let bound = self.back.0.load(Ordering::Relaxed);
        // SAFETY: A node is only relinquished after its successor link is
        // acquired. In particular, the observed tail is NEVER passed to consume:
        // an in-flight producer may still be about to write that tail's link.
        unsafe {
            while current != bound {
                let successor = next(current).load(Ordering::Acquire);
                if successor == 0 {
                    // #[gamma::skip(loop.break_to_continue, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                    break;
                }
                // #[gamma::skip(stmt.delete_call, reason = "The non-faulting prefetch is a cache hint and does not participate in queue publication")]
                crate::hal::prefetch(successor);
                let keep_going = consume(current);
                current = successor;
                if !keep_going {
                    break;
                }
            }
        }
        self.front.0.store(current, Ordering::Release);
    }
}

fn initial_drain_front(front: usize) -> Option<usize> {
    if front == 0 { None } else { Some(front) }
}

#[derive(Clone, Copy)]
struct Ring {
    first: usize,
    last: usize,
    meta: usize,
    count: u16,
}

impl Ring {
    const EMPTY: Self = Self {
        first: 0,
        last: 0,
        meta: 0,
        count: 0,
    };

    fn single(meta: usize, ptr: usize) -> Self {
        Self {
            first: ptr,
            last: ptr,
            meta,
            count: 1,
        }
    }

    /// # Safety
    /// The ring is nonempty and exclusively owns its objects. ptr is another
    /// exclusively retired object from the same slab, not already in the ring.
    unsafe fn push_nonempty(&mut self, ptr: usize) {
        // SAFETY: The existing last free object exclusively owns this link word.
        unsafe { *(self.last as *mut usize) = ptr };
        self.last = ptr;
        self.count += 1;
    }

    unsafe fn close(self) -> usize {
        let packed = (self.first.wrapping_sub(self.last) << LENGTH_BITS) | usize::from(self.count);
        // SAFETY: The final free object has at least 16 aligned bytes. Replacing
        // its raw storage initializes the message before any publication.
        unsafe {
            (self.last as *mut Message).write(Message {
                ring: packed,
                next: AtomicUsize::new(0),
            });
        };
        self.last
    }
}

#[derive(Clone, Copy)]
struct List {
    first: usize,
    last: usize,
}

impl List {
    const EMPTY: Self = Self { first: 0, last: 0 };

    unsafe fn push(&mut self, message: usize) {
        if self.first == 0 {
            self.first = message;
        } else {
            // SAFETY: The unpublished list exclusively owns its initialized tail.
            unsafe { next(self.last) }.store(message, Ordering::Relaxed);
        }
        // SAFETY: The incoming initialized message is still unpublished.
        unsafe { next(message) }.store(0, Ordering::Relaxed);
        self.last = message;
    }
}

pub(crate) struct Cache {
    rings: [Ring; 16],
    lists: [List; 256],
    remaining: usize,
}

impl Cache {
    #[cfg(test)]
    pub(crate) fn assert_cached_metadata_and_initial_routing(&self, map: Map) {
        for ring in &self.rings {
            if ring.meta != 0 {
                // SAFETY: every open ring retains its live slab and exclusively owns its first object.
                assert_eq!(ring.meta, unsafe { map.lookup(ring.first) }.0);
            }
        }
        for (index, list) in self.lists.iter().enumerate() {
            let mut message = list.first;
            while message != 0 {
                // SAFETY: unpublished messages retain their stable slab routing metadata.
                let (_, encoded) = unsafe { map.lookup(message) };
                assert_eq!(index, Self::slot(encoded & !255, 0));
                // SAFETY: this cache exclusively owns the initialized unpublished message.
                message = unsafe { next(message) }.load(Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn observe(&self, map: Map, budget: &mut usize) -> seismograph_rallocator::native::RemoteState {
        let mut result = seismograph_rallocator::native::RemoteState {
            complete: true,
            budget_remaining: self.remaining as u64,
            ..Default::default()
        };
        for ring in &self.rings {
            if ring.meta != 0 {
                result.open_rings += 1;
                result.open_objects += u64::from(ring.count);
            }
        }
        for list in &self.lists {
            if list.first != 0 {
                result.outgoing_lists += 1;
            }
            let mut message = list.first;
            while message != 0 {
                if *budget == 0 {
                    result.complete = false;
                    // #[gamma::skip(loop.break_to_continue, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                    break;
                }
                *budget -= 1;
                // SAFETY: Outgoing cache messages are exclusively owned, initialized,
                // and not yet published to another endpoint.
                let (_, count) = unsafe { open(message) };
                // SAFETY: Each cached message retains its stable frontend metadata.
                let (_, encoded) = unsafe { map.lookup(message) };
                result.messages += 1;
                result.message_objects += u64::from(count);
                result.message_bytes += crate::classes::size(encoded & 127) as u64 * u64::from(count);
                // SAFETY: The outgoing cache exclusively owns this initialized message link.
                message = unsafe { next(message) }.load(Ordering::Relaxed);
            }
        }
        result
    }

    pub(crate) const fn new() -> Self {
        Self {
            rings: [Ring::EMPTY; 16],
            lists: [List::EMPTY; 256],
            remaining: BUDGET,
        }
    }

    pub(crate) fn clear_budget(&mut self) {
        self.remaining = 0;
    }

    pub(crate) fn reserve(&mut self, bytes: usize) -> bool {
        if self.remaining > bytes {
            self.remaining -= bytes;
            true
        } else {
            false
        }
    }

    fn slot(owner: usize, round: usize) -> usize {
        (owner >> (Owner::ALLOC_BITS + round * 8)) & 255
    }

    /// # Safety
    /// The initialized ring message is exclusively owned; its slab remains
    /// frontend-owned and its pagemap owner is a persistent Owner endpoint.
    pub(crate) unsafe fn forward(&mut self, map: Map, message: usize, round: usize) {
        // SAFETY: Pending messages retain their frontend slabs and owner metadata.
        let (_, encoded) = unsafe {
            map.lookup(
                // #[gamma::skip(expr.increment, reason = "Messages occupy at least sixteen-byte-aligned live allocation starts, so adding one stays within the identical pagemap chunk")]
                message,
            )
        };
        let slot = Self::slot(
            // #[gamma::skip(literal.int_to_one, literal.int_to_zero, literal.int_decrement, literal.int_increment, reason = "Slot selection discards Owner::ALLOC_BITS low bits; owner addresses are aligned to ALLOC_SIZE, so every changed low-byte mask is discarded")]
            encoded & !255,
            round,
        );
        // SAFETY: Message ownership is transferred into the selected list.
        unsafe { self.lists[slot].push(message) };
    }

    unsafe fn close_ring(&mut self, map: Map, index: usize) {
        let ring = std::mem::replace(&mut self.rings[index], Ring::EMPTY);
        // SAFETY: A nonempty ring exclusively owns its objects and live metadata.
        let message = unsafe { ring.close() };
        // SAFETY: Closing initialized the message without releasing the slab.
        unsafe { self.forward(map, message, 0) };
    }

    /// # Safety
    /// ptr is exclusively owned, no longer live to the client, and meta is its
    /// still-live slab metadata. It is not already in any list or remote queue.
    #[inline]
    pub(crate) unsafe fn deallocate(&mut self, map: Map, meta: usize, ptr: usize) {
        // Match the pinned source's exact ring_set()+way indexing (including
        // overlapping sets), not a conventional set*associativity rewrite.
        let set = Self::ring_set(
            // #[gamma::skip(expr.increment, reason = "Slab metadata is aligned to Slab::ALIGN; adding one before division preserves the hash input")]
            meta,
        );
        // #[gamma::skip(range.exclusive_to_inclusive, expr.increment, literal.int_increment, reason = "The extra third way cannot hold this metadata: insertion of this same slab is restricted to its first two ways")]
        for index in set..set + 2 {
            if self.rings[index].meta == meta {
                // SAFETY: Live slab metadata is nonzero, so a match also
                // proves this ring is nonempty and owns same-slab objects.
                unsafe { self.rings[index].push_nonempty(ptr) };
                return;
            }
        }
        // SAFETY: The uniquely retired object has no matching cached ring.
        unsafe { self.deallocate_miss(map, meta, ptr) };
    }

    fn ring_set(meta: usize) -> usize {
        ((meta / crate::core::Slab::ALIGN).wrapping_mul(0x7efb_352d) >> 16) & 7
    }

    // Victim closing made every hit save four nonvolatile registers. Recompute
    // the small hash here rather than adding a fifth Win64 stack argument.
    #[inline(never)]
    unsafe fn deallocate_miss(&mut self, map: Map, meta: usize, ptr: usize) {
        let set = Self::ring_set(
            // #[gamma::skip(expr.increment, reason = "Slab metadata is aligned to Slab::ALIGN; adding one before division preserves the hash input")]
            meta,
        );
        let mut victim = set;
        for index in set..set + 2 {
            if self.rings[index].meta == 0 {
                victim = index;
                break;
            }
            if self.rings[index].count > self.rings[victim].count {
                victim = index;
            }
        }
        if self.rings[victim].meta != 0 {
            // SAFETY: The nonempty victim ring is exclusively owned.
            unsafe { self.close_ring(map, victim) };
        }
        self.rings[victim] = Ring::single(meta, ptr);
    }

    /// # Safety
    /// Caller is the current owner's unique lease holder; every list message
    /// targets a different persistent endpoint. All objects remain unpublished.
    pub(crate) unsafe fn post(&mut self, map: Map, owner: usize) {
        for i in 0..self.rings.len() {
            if self.rings[i].meta != 0 {
                // SAFETY: Each nonempty cache ring is exclusively owned.
                unsafe { self.close_ring(map, i) };
            }
        }
        let mut round = 0;
        loop {
            let own_slot = Self::slot(
                // #[gamma::skip(expr.increment, reason = "Owner endpoints are ALLOC_SIZE-aligned and slot selection discards all ALLOC_BITS low address bits")]
                owner, round,
            );
            for i in 0..256 {
                if i == own_slot || self.lists[i].first == 0 {
                    continue;
                }
                let list = std::mem::replace(&mut self.lists[i], List::EMPTY);
                // SAFETY: The first initialized message retains its frontend slab.
                let (_, encoded) = unsafe { map.lookup(list.first) };
                // SAFETY: The pagemap identifies a permanent owner endpoint.
                let target = unsafe { &*((encoded & !255) as *const Owner) };
                // SAFETY: The initialized segment transfers once to the target queue.
                unsafe { target.queue().enqueue(list.first, list.last) };
            }
            let list = std::mem::replace(&mut self.lists[own_slot], List::EMPTY);
            if list.first == 0 {
                // #[gamma::skip(loop.break_to_continue, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                break;
            }
            // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            round +=
            // #[gamma::skip(literal.int_decrement, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            1;
            let mut message = list.first;
            while message != 0 {
                // SAFETY: This unpublished list exclusively owns the initialized message.
                let successor = unsafe { next(message) }.load(Ordering::Relaxed);
                // SAFETY: Save the successor before relinking this retained message.
                unsafe { self.forward(map, message, round) };
                // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                message = successor;
            }
        }
        self.remaining = BUDGET;
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.rings.iter().all(|ring| ring.meta == 0) && self.lists.iter().all(|list| list.first == 0 && list.last == 0)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn gamma_round2_colliding_owner_slots_are_rerouted_before_publication() {
        let map = crate::backend::map().unwrap();
        // SAFETY: this private allocator uses the unique registered backend map.
        let mut backend = unsafe { crate::backend::Local::new(map) };
        let envelope = backend.alloc_meta(1 << 24);
        assert_ne!(envelope, 0);
        let addresses = [
            envelope + Owner::ALLOC_SIZE,
            envelope + (1 << 22) + Owner::ALLOC_SIZE,
            envelope + (2 << 22) + Owner::ALLOC_SIZE,
        ];
        for &address in &addresses {
            // SAFETY: each aligned committed envelope is disjoint, initialized once,
            // and withheld permanently from the larger backend allocation.
            unsafe { Owner::initialize(map, address) };
        }
        let [left, right, sender] = addresses;
        assert_eq!(Cache::slot(left, 0), Cache::slot(right, 0));
        assert_eq!(Cache::slot(left, 0), Cache::slot(sender, 0));
        assert_ne!(Cache::slot(left, 1), Cache::slot(right, 1));
        assert_ne!(Cache::slot(left, 1), Cache::slot(sender, 1));
        assert_ne!(Cache::slot(right, 1), Cache::slot(sender, 1));
        let request = crate::classes::Request::new(std::alloc::Layout::from_size_align(32, 16).unwrap()).unwrap();
        let mut cache = Cache::new();
        let mut messages = Vec::new();
        for address in [left, right] {
            // SAFETY: the initialized endpoint is persistent and exclusively leased here.
            let owner = unsafe { &*(address as *const Owner) };
            // SAFETY: no other reference borrows this owner's core.
            let core = unsafe { &mut *owner.core_ptr() };
            let pointer = core.allocate(request);
            assert!(!pointer.is_null());
            // SAFETY: this live allocation retains its registered slab metadata.
            let (meta, _) = unsafe { map.lookup(pointer.addr()) };
            // SAFETY: each allocation transfers exactly once into the private remote cache.
            unsafe { cache.deallocate(map, meta, pointer.addr()) };
            messages.push(pointer.addr());
        }
        // SAFETY: both messages target other persistent owners, not the sender.
        unsafe { cache.post(map, sender) };
        assert!(cache.is_empty());
        for (address, message) in [left, right].into_iter().zip(messages) {
            // SAFETY: the endpoint remains initialized and persistent.
            let owner = unsafe { &*(address as *const Owner) };
            assert_eq!(owner.queue().observe(), (message as u64, message as u64));
        }
    }

    #[test]
    fn gamma_round2_forwarded_list_keeps_the_exact_message_address() {
        let (map, target, sender) = gamma_survivor_remote_endpoints();
        // SAFETY: the fixture initialized a persistent endpoint exclusively leased here.
        let owner = unsafe { &*(target as *const Owner) };
        // SAFETY: this test is the only core borrower.
        let core = unsafe { &mut *owner.core_ptr() };
        let request = crate::classes::Request::new(std::alloc::Layout::from_size_align(32, 16).unwrap()).unwrap();
        let pointer = core.allocate(request);
        assert!(!pointer.is_null());
        // SAFETY: the live allocation retains its slab metadata.
        let (meta, _) = unsafe { map.lookup(pointer.addr()) };
        // SAFETY: the single object is exclusively retired into this ring.
        let message = unsafe { Ring::single(meta, pointer.addr()).close() };
        let mut cache = Cache::new();
        // SAFETY: the initialized message is owned by this test and is not published.
        unsafe { cache.forward(map, message, 0) };
        let slot = Cache::slot(target, 0);
        assert_eq!((cache.lists[slot].first, cache.lists[slot].last), (message, message));
        // SAFETY: the retained message targets the other persistent endpoint.
        unsafe { cache.post(map, sender) };
        assert_eq!(owner.queue().observe(), (message as u64, message as u64));
    }

    #[test]
    fn gamma_round2_drain_front_plan_rejects_empty_before_reading_any_successor() {
        assert_eq!(initial_drain_front(0), None);
        assert_eq!(initial_drain_front(1), Some(1));
        assert_eq!(initial_drain_front(16), Some(16));
    }

    #[test]
    fn gamma_round2_second_way_hit_and_larger_second_way_victim() {
        let (map, target, sender) = gamma_survivor_remote_endpoints();
        // SAFETY: The fixture initializes a persistent private target endpoint.
        let owner = unsafe { &*(target as *const Owner) };
        // SAFETY: No other caller leases this endpoint's core.
        let core = unsafe { &mut *owner.core_ptr() };
        let mut groups: [Vec<_>; 8] = std::array::from_fn(|_| Vec::new());
        for class in crate::classes::CLASSES {
            let request = crate::classes::Request::new(std::alloc::Layout::from_size_align(class.size, 16).unwrap()).unwrap();
            let pointer = core.allocate(request).addr();
            assert_ne!(pointer, 0);
            // SAFETY: The allocated object's slab is live with immutable routing metadata.
            let meta = unsafe { map.lookup(pointer) }.0;
            groups[Cache::ring_set(meta)].push((meta, pointer, request));
        }
        let group = groups.iter().find(|group| group.len() >= 3).unwrap();
        let (first, second, third) = (group[0], group[1], group[2]);
        let extra = core.allocate(second.2).addr();
        assert_ne!(extra, 0);
        // SAFETY: The second class has ample space in its one still-live slab.
        assert_eq!(unsafe { map.lookup(extra) }.0, second.0);
        let set = Cache::ring_set(first.0);
        let mut cache = Cache::new();
        // SAFETY: Transfer each distinct, uniquely owned frontend object exactly once.
        unsafe { cache.deallocate(map, first.0, first.1) };
        // SAFETY: The second object's still-live metadata collides with the first object's set.
        unsafe { cache.deallocate(map, second.0, second.1) };
        // SAFETY: This separate second-slab object must hit the second cache way.
        unsafe { cache.deallocate(map, second.0, extra) };
        assert_eq!((cache.rings[set].meta, cache.rings[set].count), (first.0, 1));
        assert_eq!((cache.rings[set + 1].meta, cache.rings[set + 1].count), (second.0, 2));
        // SAFETY: This distinct object transfers once and forces eviction from the full colliding set.
        unsafe { cache.deallocate(map, third.0, third.1) };
        assert_eq!((cache.rings[set].meta, cache.rings[set].count), (first.0, 1));
        assert_eq!((cache.rings[set + 1].meta, cache.rings[set + 1].count), (third.0, 1));
        // SAFETY: All cached objects target the other persistent endpoint.
        unsafe { cache.post(map, sender) };
        assert!(cache.is_empty());
        assert_eq!(cache.remaining, BUDGET);
        for &(meta, pointer, _) in groups.iter().flatten() {
            if ![first.0, second.0, third.0].contains(&meta) {
                // SAFETY: Each unselected live object remains uniquely owned and returns once.
                unsafe { core.deallocate(pointer) };
            }
        }
        core.flush();
    }

    #[test]
    fn gamma_survivor_queue_retains_tail_and_stops_after_callback() {
        let queue = Queue::new();
        let messages: Vec<_> = (0..5)
            .map(|_| {
                Box::new(Message {
                    ring: 1,
                    next: AtomicUsize::new(0),
                })
            })
            .collect();
        let addresses: Vec<_> = messages.iter().map(|message| (&raw const **message).addr()).collect();
        assert!(!queue.can_dequeue());
        // SAFETY: Empty queue, exclusively held by this sole consumer.
        unsafe { queue.drain(|_| panic!("empty queue must not call consume")) };
        for pair in addresses.windows(2).take(3) {
            // SAFETY: Before publication, the test owns both initialized message nodes.
            unsafe { next(pair[0]) }.store(pair[1], Ordering::Relaxed);
        }
        // A stale end link must be reset by enqueue rather than becoming a successor.
        // SAFETY: Last is an unpublished initialized message.
        unsafe { next(addresses[3]) }.store(addresses[4], Ordering::Relaxed);
        // SAFETY: Stable initialized segment transfers once to this stationary queue.
        unsafe { queue.enqueue(addresses[0], addresses[3]) };
        assert!(queue.can_dequeue());
        // SAFETY: The queued initialized tail remains retained by the single consumer.
        assert_eq!(unsafe { next(addresses[3]) }.load(Ordering::Relaxed), 0);
        let mut seen = Vec::new();
        // SAFETY: Callback only records ownership transfers and does not recursively consume.
        unsafe {
            queue.drain(|address| {
                seen.push(address);
                false
            });
        };
        assert_eq!(seen, [addresses[0]]);
        assert_eq!(queue.observe(), (addresses[1] as u64, addresses[3] as u64));
        // SAFETY: Same sole consumer with all backing still live.
        unsafe {
            queue.drain(|address| {
                seen.push(address);
                true
            });
        };
        assert_eq!(seen, addresses[..3]);
        assert!(!queue.can_dequeue());
        // SAFETY: The new stable message is exclusively held and not previously published.
        unsafe { queue.enqueue(addresses[4], addresses[4]) };
        // SAFETY: This remains the sole consumer.
        unsafe {
            queue.drain(|address| {
                seen.push(address);
                true
            });
        };
        assert_eq!(seen, addresses[..4]);
        assert_eq!(queue.retained_message(), Some(addresses[4]));
        assert!(!queue.can_dequeue());
    }

    #[test]
    fn gamma_survivor_remote_budget_requires_strictly_more_bytes() {
        let mut cache = Cache::new();
        assert!(!cache.reserve(BUDGET));
        assert_eq!(cache.remaining, BUDGET);
        assert!(cache.reserve(BUDGET - 1));
        assert_eq!(cache.remaining, 1);
        assert!(!cache.reserve(1));
        assert_eq!(cache.remaining, 1);
        assert!(cache.reserve(0));
        cache.clear_budget();
        assert!(!cache.reserve(0));
        assert_eq!(cache.remaining, 0);
        for (address, round, expected) in [(0x1234_5600usize, 0, 209), (0x1234_5600, 1, 72), (0x1234_5600, 2, 0)] {
            assert_eq!(Cache::slot(address, round), expected);
        }
        for (meta, expected) in [
            (8, 3),
            (16, 6),
            (24, 1),
            (32, 4),
            (0x1000, 2),
            (0x8000, 2),
            (0x1234_5678, 0),
            (0x1234_5678_9ab8, 4),
            (0x007f_ffff_ffff_fff8, 4),
        ] {
            assert_eq!(Cache::ring_set(meta), expected);
        }
    }

    #[test]
    fn gamma_survivor_ring_close_links_and_list_order() {
        let mut objects = [0usize; 32];
        let base = objects.as_mut_ptr().addr();
        for order in [[0, 32, 64], [64, 32, 0]] {
            let mut ring = Ring::single(8, base + order[0]);
            for offset in &order[1..] {
                // SAFETY: Every 32-byte object is distinct and exclusively writable.
                unsafe { ring.push_nonempty(base + offset) };
            }
            // SAFETY: The nonempty ring owns all its object storage.
            let message = unsafe { ring.close() };
            // SAFETY: Closing initialized the message and its next atomic.
            assert_eq!(unsafe { next(message) }.load(Ordering::Relaxed), 0);
            // SAFETY: The ring word remains initialized and immutable.
            assert_eq!(unsafe { open(message) }, (base + order[0], 3));
        }
        let a = Box::new(Message {
            ring: 1,
            next: AtomicUsize::new(0),
        });
        let b = Box::new(Message {
            ring: 1,
            next: AtomicUsize::new(0),
        });
        let mut list = List::EMPTY;
        let a = (&raw const *a).addr();
        let b = (&raw const *b).addr();
        // SAFETY: This initialized message is exclusively test-owned and not published.
        unsafe { list.push(a) };
        // SAFETY: The other initialized message is distinct and not yet published.
        unsafe { list.push(b) };
        assert_eq!((list.first, list.last), (a, b));
        // SAFETY: The list exclusively owns its initialized links.
        assert_eq!(unsafe { next(a) }.load(Ordering::Relaxed), b);
        // SAFETY: The list tail is initialized and exclusively held.
        assert_eq!(unsafe { next(b) }.load(Ordering::Relaxed), 0);
    }

    fn gamma_survivor_remote_endpoints() -> (Map, usize, usize) {
        let map = crate::backend::map().unwrap();
        // SAFETY: This private metadata allocator uses the backend's unique registered map.
        let mut backend = unsafe { crate::backend::Local::new(map) };
        let target = backend.alloc_meta(Owner::ALLOC_SIZE);
        let sender = backend.alloc_meta(Owner::ALLOC_SIZE);
        assert_ne!(target, 0);
        assert_ne!(sender, 0);
        // SAFETY: This fresh committed envelope is initialized once and remains persistent.
        unsafe { Owner::initialize(map, target) };
        // SAFETY: The other envelope is disjoint, committed, and initialized once.
        unsafe { Owner::initialize(map, sender) };
        (map, target, sender)
    }

    #[test]
    fn gamma_survivor_two_way_ring_hits_victims_and_routing_rounds() {
        let (map, target, sender) = gamma_survivor_remote_endpoints();
        // SAFETY: The fixture initialized this persistent endpoint.
        let owner = unsafe { &*(target as *const Owner) };
        // SAFETY: The test holds the target's unique core lease for all allocations and returns.
        let core = unsafe { &mut *owner.core_ptr() };
        let mut groups = [
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ];
        for class in crate::classes::CLASSES {
            let request = crate::classes::Request::new(std::alloc::Layout::from_size_align(class.size, 16).unwrap()).unwrap();
            let pointer = core.allocate(request).addr();
            assert_ne!(pointer, 0);
            // SAFETY: The just-allocated frontend object keeps its slab metadata live.
            let (meta, _) = unsafe { map.lookup(pointer) };
            groups[Cache::ring_set(meta)].push((meta, pointer, request));
        }
        let group = groups.iter().find(|group| group.len() >= 3).unwrap();
        let (first, second, third) = (group[0], group[1], group[2]);
        let extra = core.allocate(first.2).addr();
        assert_ne!(extra, 0);
        // SAFETY: The same class's first slab has many unused objects and remains live.
        assert_eq!(unsafe { map.lookup(extra) }.0, first.0);
        let mut tied = Cache::new();
        let tied_first = core.allocate(first.2).addr();
        let tied_second = core.allocate(second.2).addr();
        let tied_third = core.allocate(third.2).addr();
        assert!(tied_first != 0 && tied_second != 0 && tied_third != 0);
        // SAFETY: Each new allocation is live in its existing target slab.
        assert_eq!(unsafe { map.lookup(tied_first) }.0, first.0);
        // SAFETY: The second target allocation remains live and immutable.
        assert_eq!(unsafe { map.lookup(tied_second) }.0, second.0);
        // SAFETY: The third target allocation remains live and immutable.
        assert_eq!(unsafe { map.lookup(tied_third) }.0, third.0);
        // SAFETY: This live object transfers once to the independent cache.
        unsafe { tied.deallocate(map, first.0, tied_first) };
        // SAFETY: The second live object is distinct and transferred once.
        unsafe { tied.deallocate(map, second.0, tied_second) };
        // SAFETY: The third live object is also distinct and transferred once.
        unsafe { tied.deallocate(map, third.0, tied_third) };
        let set = Cache::ring_set(first.0);
        assert_eq!(tied.rings[set].meta, third.0);
        assert_eq!(tied.rings[set + 1].meta, second.0);
        // SAFETY: The tie-case cache owns all pending messages for the different stable target.
        unsafe { tied.post(map, sender) };
        let mut cache = Cache::new();
        // SAFETY: This live object belongs to the target's still-live frontend slab.
        unsafe { cache.deallocate(map, first.0, first.1) };
        // SAFETY: The extra live object is distinct and belongs to the same slab.
        unsafe { cache.deallocate(map, first.0, extra) };
        // SAFETY: This other live object transfers once from its own still-live slab.
        unsafe { cache.deallocate(map, second.0, second.1) };
        assert_eq!((cache.rings[set].meta, cache.rings[set].count), (first.0, 2));
        assert_eq!((cache.rings[set + 1].meta, cache.rings[set + 1].count), (second.0, 1));
        // SAFETY: This third distinct retired object's metadata collides with the full two-way set.
        unsafe { cache.deallocate(map, third.0, third.1) };
        assert_eq!((cache.rings[set].meta, cache.rings[set].count), (third.0, 1));
        assert_eq!(cache.rings[set + 1].meta, second.0);
        let routed = (target >> 14) & 255;
        assert_ne!(cache.lists[routed].first, 0);
        // SAFETY: The only closed list is still unpublished and exclusively held.
        let (_, count) = unsafe { open(cache.lists[routed].first) };
        assert_eq!(count, 2);
        let mut budget = usize::MAX;
        let observed = cache.observe(map, &mut budget);
        assert_eq!(
            (
                observed.open_rings,
                observed.open_objects,
                observed.messages,
                observed.message_objects
            ),
            (2, 2, 1, 2)
        );
        assert_eq!(observed.message_bytes, first.2.size() as u64 * 2);
        let message = std::mem::replace(&mut cache.lists[routed], List::EMPTY).first;
        for round in 0..=5 {
            // SAFETY: This initialized message is unpublished, with live target metadata.
            unsafe { cache.forward(map, message, round) };
            let slot = (target >> (14 + round * 8)) & 255;
            assert_eq!((cache.lists[slot].first, cache.lists[slot].last), (message, message));
            cache.lists[slot] = List::EMPTY;
        }
        // SAFETY: Restore normal round-zero routing before handing the cache to post.
        unsafe { cache.forward(map, message, 0) };
        assert!(cache.reserve(32));
        assert_eq!(cache.remaining, BUDGET - 32);
        // SAFETY: post transfers all cache-owned messages to the different persistent target endpoint.
        unsafe { cache.post(map, sender) };
        assert!(cache.is_empty());
        assert_eq!(cache.remaining, BUDGET);
        for &(meta, pointer, _) in groups.iter().flatten() {
            if [first.0, second.0, third.0].contains(&meta) {
                continue;
            }
            // SAFETY: All unselected objects remained live; each is retired once by its owning core.
            unsafe { core.deallocate(pointer) };
        }
        core.flush();
    }

    #[test]
    fn gamma_survivor_closed_ring_observation_budget() {
        let (map, target, sender) = gamma_survivor_remote_endpoints();
        // SAFETY: The fixture initialized this persistent endpoint.
        let owner = unsafe { &*(target as *const Owner) };
        // SAFETY: The test holds this fresh endpoint's unique core lease.
        let core = unsafe { &mut *owner.core_ptr() };
        let mut cache = Cache::new();
        let request = crate::classes::Request::new(std::alloc::Layout::from_size_align(32, 16).unwrap()).unwrap();
        let first = core.allocate(request).addr();
        let second = core.allocate(request).addr();
        assert_ne!(first, 0);
        assert_ne!(second, 0);
        // SAFETY: Both new live frontend objects retain their metadata.
        let meta = unsafe { map.lookup(first) }.0;
        // SAFETY: Transfer this live object once into the empty cache.
        unsafe { cache.deallocate(map, meta, first) };
        // SAFETY: The second live object is distinct and transferred once.
        unsafe { cache.deallocate(map, meta, second) };
        // SAFETY: The known nonempty ring exclusively owns both retired objects.
        unsafe { cache.close_ring(map, Cache::ring_set(meta)) };
        let mut budget = 1;
        let observed = cache.observe(map, &mut budget);
        assert_eq!((observed.messages, observed.message_objects, observed.message_bytes), (1, 2, 64));
        assert_eq!(budget, 0);
        let mut exhausted = 0;
        let observed = cache.observe(map, &mut exhausted);
        assert!(!observed.complete);
        assert_eq!(observed.messages, 0);
        // SAFETY: Post these cache-owned messages once to their distinct stable endpoint.
        unsafe { cache.post(map, sender) };
        core.flush();
    }

    #[test]
    fn routing_rounds_use_logical_owner_envelope() {
        assert_eq!(Owner::ALLOC_BITS, 14);
        for round in 0..=(usize::BITS as usize - 1 - 14) / 8 {
            let shift = 14 + round * 8;
            let available_bits = (usize::BITS as usize - shift).min(8);
            for slot in 0..1usize << available_bits {
                let address = slot << shift;
                assert_eq!(Cache::slot(address, round), slot);
                if round != 0 {
                    assert_eq!(Cache::slot(address, round - 1), 0);
                }
            }
        }
    }

    #[test]
    fn stalled_publisher_cannot_expose_or_reclaim_tail() {
        let queue = Arc::new(Queue::new());
        assert_eq!(queue.observe(), (0, 0));
        let first = Box::new(Message {
            ring: 1,
            next: AtomicUsize::new(0),
        });
        let second = Box::new(Message {
            ring: 1,
            next: AtomicUsize::new(0),
        });
        let a = (&raw const *first) as usize;
        let b = (&raw const *second) as usize;
        // SAFETY: Stable initialized objects, exclusively owned before publishing.
        unsafe { queue.enqueue(a, a) };
        assert_eq!(queue.observe(), (a as u64, a as u64));
        let swapped = Arc::new(Barrier::new(2));
        let resume = Arc::new(Barrier::new(2));
        let producer = {
            let queue = Arc::clone(&queue);
            let swapped = Arc::clone(&swapped);
            let resume = Arc::clone(&resume);
            std::thread::spawn(move || {
                // Reproduce a preemption precisely between exchange and link.
                let previous = queue.back.0.swap(b, Ordering::AcqRel);
                swapped.wait();
                assert_eq!(queue.observe(), (a as u64, b as u64));
                resume.wait();
                // SAFETY: Consumer is required to retain previous until linked.
                unsafe { next(previous).store(b, Ordering::Release) };
            })
        };
        swapped.wait();
        let mut seen = Vec::new();
        let mut accept = |message| {
            seen.push(message);
            true
        };
        // SAFETY: This test is the only consumer throughout the queue's lifetime.
        unsafe {
            queue.drain(&mut accept);
        };
        assert_eq!(queue.retained_message(), Some(a));
        resume.wait();
        producer.join().unwrap();
        // SAFETY: Same sole consumer; the acquired successor makes only a
        // reclaimable. b remains the user tail until another real message arrives.
        unsafe {
            queue.drain(&mut accept);
            assert_eq!(seen, [a]);
            assert_eq!(queue.retained_message(), Some(b));
            assert!(!queue.can_dequeue());
        }
        let third = Box::new(Message {
            ring: 1,
            next: AtomicUsize::new(0),
        });
        let c = (&raw const *third) as usize;
        // SAFETY: c is a new initialized stable message. Publishing it links
        // b and allows this sole consumer to reclaim b, while retaining c.
        unsafe { queue.enqueue(c, c) };
        // SAFETY: This remains the sole consumer; the successor makes b reclaimable.
        unsafe {
            queue.drain(|message| {
                seen.push(message);
                true
            });
        }
        assert_eq!(seen, [a, b]);
        assert_eq!(queue.retained_message(), Some(c));
        assert_eq!(queue.observe(), (c as u64, c as u64));
    }

    #[test]
    fn mpsc_batched_publication_during_drain() {
        const PRODUCERS: usize = 8;
        const PER_PRODUCER: usize = 8192;
        let queue = Arc::new(Queue::new());
        let mut messages = (0..PRODUCERS * PER_PRODUCER)
            .map(|_| Message {
                ring: 1,
                next: AtomicUsize::new(0),
            })
            .collect::<Vec<_>>();
        let base = messages.as_mut_ptr() as usize;
        let start = Arc::new(Barrier::new(PRODUCERS + 1));
        let completed = Arc::new(AtomicUsize::new(0));
        let workers = (0..PRODUCERS)
            .map(|producer| {
                let queue = Arc::clone(&queue);
                let start = Arc::clone(&start);
                let completed = Arc::clone(&completed);
                std::thread::spawn(move || {
                    start.wait();
                    for offset in (0..PER_PRODUCER).step_by(8) {
                        let first = base + (producer * PER_PRODUCER + offset) * std::mem::size_of::<Message>();
                        let last = first + 7 * std::mem::size_of::<Message>();
                        for i in 0..7 {
                            let address = first + i * std::mem::size_of::<Message>();
                            // SAFETY: This producer exclusively owns its initialized segment.
                            unsafe { next(address) }.store(address + std::mem::size_of::<Message>(), Ordering::Relaxed);
                        }
                        // SAFETY: Every segment link is initialized before publication.
                        unsafe { queue.enqueue(first, last) };
                    }
                    completed.fetch_add(1, Ordering::Release);
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        let mut seen = vec![false; messages.len()];
        let mut accept = |address: usize| {
            let index = (address - base) / std::mem::size_of::<Message>();
            assert!(!seen[index], "message returned twice");
            seen[index] = true;
            true
        };
        // Drain at least once even if every producer finishes before this thread resumes.
        let mut keep_draining = true;
        while keep_draining {
            // SAFETY: This test thread is the single queue consumer.
            unsafe { queue.drain(&mut accept) };
            std::thread::yield_now();
            keep_draining = completed.load(Ordering::Acquire) != PRODUCERS;
        }
        for worker in workers {
            worker.join().unwrap();
        }
        // SAFETY: All publishers joined; this remains the single consumer.
        // Native draining still retains the last real message.
        unsafe {
            queue.drain(&mut accept);
        }
        let tail = queue.retained_message().unwrap();
        let retained_index = (tail - base) / std::mem::size_of::<Message>();
        assert!(!queue.can_dequeue());
        assert!(
            seen.into_iter()
                .enumerate()
                .all(|(index, value)| value == (index != retained_index))
        );
    }

    #[test]
    fn ring_displacement_handles_negative_and_positive_offsets() {
        let mut objects = [0usize; 32];
        let base = objects.as_mut_ptr() as usize;
        for order in [[0usize, 32, 64, 96], [96, 64, 32, 0]] {
            let mut ring = Ring::single(1, base + order[0]);
            for &offset in &order[1..] {
                // SAFETY: Each object occupies 32 disjoint bytes in live aligned storage.
                unsafe { ring.push_nonempty(base + offset) };
            }
            // SAFETY: The ring exclusively owns four objects with message-sized storage.
            let message = unsafe { ring.close() };
            // SAFETY: Closing initialized the message's immutable ring word.
            let (mut current, count) = unsafe { open(message) };
            assert_eq!(count, 4);
            for offset in &order[..3] {
                assert_eq!(current, base + offset);
                // SAFETY: Each non-tail object has an initialized ordinary link.
                current = unsafe { *(current as *const usize) };
            }
            assert_eq!(current, message);
        }
    }
    #[test]
    fn outgoing_observation_counts_closed_messages_and_marks_budget_truncation() {
        let map = Map::reserve().unwrap();
        let size = crate::hal::RESERVE_MIN;
        let address = crate::hal::reserve(size, size) as usize;
        assert_ne!(address, 0);
        // SAFETY: The test exclusively owns and registers this complete reservation.
        assert!(unsafe { map.register(address, size) });
        // SAFETY: The complete owned reservation is committed before writing any messages.
        assert!(unsafe { crate::hal::commit(address as *mut u8, size) });
        // SAFETY: The registered range remains exclusively owned by the test.
        unsafe { map.assign(address, size, 0x1000, 0x2040) };
        let mut cache = Cache::new();
        // SAFETY: This message-sized object belongs to the registered test slab.
        unsafe { cache.deallocate(map, 0x1000, address) };
        // SAFETY: The second message-sized object is disjoint and belongs to the same slab.
        unsafe { cache.deallocate(map, 0x1000, address + 32) };
        let open = cache.observe(map, &mut 1);
        assert_eq!((open.open_rings, open.open_objects, open.messages), (1, 2, 0));
        assert_eq!(open.budget_remaining, BUDGET as u64);
        assert!(cache.reserve(32));
        let index = Cache::ring_set(0x1000);
        // SAFETY: Closing transfers this test's exclusive initialized ring to an outgoing list.
        unsafe { cache.close_ring(map, index) };
        let complete = cache.observe(map, &mut 1);
        assert_eq!(
            (
                complete.open_rings,
                complete.outgoing_lists,
                complete.messages,
                complete.message_objects,
                complete.message_bytes
            ),
            (0, 1, 1, 2, 32)
        );
        assert!(complete.complete);
        assert_eq!(complete.budget_remaining, (BUDGET - 32) as u64);
        let partial = cache.observe(map, &mut 0);
        assert!(!partial.complete);
        assert_eq!((partial.outgoing_lists, partial.messages), (1, 0));
        assert_eq!(partial.budget_remaining, complete.budget_remaining);
        cache.clear_budget();
        assert_eq!(cache.observe(map, &mut 1).budget_remaining, 0);
        // SAFETY: No queue publication occurred; the test retires all ring storage together.
        unsafe { crate::hal::release(address as *mut u8, size) };
    }
}
