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

    pub(crate) fn can_dequeue(&self) -> bool {
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
        let mut current = self.front.0.load(Ordering::Acquire);
        if current == 0 {
            return;
        }
        let bound = self.back.0.load(Ordering::Relaxed);
        // SAFETY: A node is only relinquished after its successor link is
        // acquired. In particular, the observed tail is NEVER passed to consume:
        // an in-flight producer may still be about to write that tail's link.
        unsafe {
            while current != bound {
                let successor = next(current).load(Ordering::Acquire);
                if successor == 0 {
                    break;
                }
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
        let (_, encoded) = unsafe { map.lookup(message) };
        let slot = Self::slot(encoded & !255, round);
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
        let set = Self::ring_set(meta);
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
        let set = Self::ring_set(meta);
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
            let own_slot = Self::slot(owner, round);
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
                break;
            }
            round += 1;
            let mut message = list.first;
            while message != 0 {
                // SAFETY: This unpublished list exclusively owns the initialized message.
                let successor = unsafe { next(message) }.load(Ordering::Relaxed);
                // SAFETY: Save the successor before relinking this retained message.
                unsafe { self.forward(map, message, round) };
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
                resume.wait();
                // SAFETY: Consumer is required to retain previous until linked.
                unsafe { next(previous).store(b, Ordering::Release) };
            })
        };
        swapped.wait();
        let mut seen = Vec::new();
        // SAFETY: This test is the only consumer throughout the queue's lifetime.
        unsafe {
            queue.drain(|message| {
                seen.push(message);
                true
            });
        };
        assert!(seen.is_empty());
        resume.wait();
        producer.join().unwrap();
        // SAFETY: Same sole consumer; the acquired successor makes only a
        // reclaimable. b remains the user tail until another real message arrives.
        unsafe {
            queue.drain(|message| {
                seen.push(message);
                true
            });
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
        while completed.load(Ordering::Acquire) != PRODUCERS {
            // SAFETY: This test thread is the single queue consumer.
            unsafe { queue.drain(&mut accept) };
            std::thread::yield_now();
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
}
