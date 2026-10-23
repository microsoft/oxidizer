// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::mem::replace;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering, fence};
use std::task::Waker;

use crate::sync::raw_mutex::RawMutex;
use crate::sync::slot_list::{Key, SlotList};

// Watch wakes every subscriber, so it does not need a globally ordered FIFO.
// Eight cache-line-separated shards avoid one registration/cancellation lock;
// indexed slots avoid both a waiter allocation and an O(n) cancellation scan.
const SHARDS: usize = 8;
const _: () = assert!(
    SHARDS.is_power_of_two(),
    "watch shard bitmask selection requires a power-of-two shard count"
);

// This is only a load-balancing hint, never ownership or channel identity.
// Allocate a lane once per worker instead of contending on a selector on every
// registration. A moved future retains its original registration's lane.
static NEXT_SHARD: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static THREAD_SHARD: usize = NEXT_SHARD.fetch_add(1, Ordering::Relaxed) & (SHARDS - 1);
}

/// Watch's sharded registration, cancellation and broadcast-wake storage.
#[derive(Debug)]
pub(super) struct Broadcast {
    shards: [Shard; SHARDS],
}

impl Broadcast {
    pub(super) const fn new() -> Self {
        Self {
            shards: [const { Shard::new() }; SHARDS],
        }
    }

    /// Inserts or refreshes a wait, then returns whether readiness won the race.
    ///
    /// A token can outlive a broadcast drain. Refreshing recreates its slot when
    /// necessary; cancellation tolerates a drained token and rejects reused slots.
    pub(super) fn register(&self, registration: &mut Option<Registration>, waker: &Waker, ready: impl FnOnce() -> bool) -> bool {
        let shard_index = registration.map_or_else(|| THREAD_SHARD.with(|shard| *shard), |registration| registration.shard);
        let shard = &self.shards[shard_index];
        // Custom waker cloning and destruction may re-enter the channel.
        let replacement = waker.clone();
        let mut waiters = shard.waiters.lock();
        let previous = if let Some(registered) = registration
            && let Some(waker) = waiters.get_mut(registered.key)
        {
            Some(replace(waker, replacement))
        } else {
            let key = waiters.push_back(replacement);
            *registration = Some(Registration { shard: shard_index, key });
            None
        };
        // Paired with the publication fence in wake_all: either this recheck
        // sees the producer's released state or the producer sees pending.
        // One fence avoids an RMW on every shard of an empty broadcast.
        shard.pending.store(true, Ordering::Relaxed);
        fence(Ordering::SeqCst);
        let discarded = if ready() {
            let registered = registration.take().expect("registration inserted or refreshed a waiter above");
            let discarded = waiters.remove(registered.key);
            if waiters.is_empty() {
                shard.pending.store(false, Ordering::Release);
            }
            discarded
        } else {
            drop(waiters);
            drop(previous);
            return false;
        };
        drop(waiters);
        drop(previous);
        drop(discarded);
        true
    }

    pub(super) fn cancel(&self, registration: Registration) {
        let shard = &self.shards[registration.shard];
        // This registration published true before it could escape register().
        // False therefore means its slot was drained, not merely hinted empty.
        if !shard.pending.load(Ordering::Acquire) {
            return;
        }
        let mut waiters = shard.waiters.lock();
        let discarded = waiters.remove(registration.key);
        if waiters.is_empty() {
            shard.pending.store(false, Ordering::Release);
        }
        drop(waiters);
        drop(discarded);
    }

    pub(super) fn wake_all(&self) {
        // The caller publishes its version/closure before this fence.
        fence(Ordering::SeqCst);
        for shard in &self.shards {
            if !shard.pending.load(Ordering::Acquire) {
                continue;
            }
            let wakers = {
                let mut waiters = shard.waiters.lock();
                let mut wakers = Vec::with_capacity(waiters.len());
                while let Some(waker) = waiters.pop_front() {
                    wakers.push(waker);
                }
                shard.pending.store(false, Ordering::Release);
                wakers
            };
            for waker in wakers {
                waker.wake();
            }
        }
    }

    #[cfg(test)]
    pub(super) fn has_waiters(&self) -> bool {
        self.shards.iter().any(|shard| shard.pending.load(Ordering::Acquire))
    }
}

/// Future-owned shard/key token that may outlive its queued waker.
#[derive(Clone, Copy, Debug)]
pub(super) struct Registration {
    shard: usize,
    key: Key,
}

/// One cache-separated waiter partition protected by its own lock.
#[derive(Debug)]
#[repr(align(64))]
struct Shard {
    pending: AtomicBool,
    waiters: RawMutex<SlotList<Waker>>,
}

impl Shard {
    #[cfg_attr(coverage_nightly, coverage(off))] // Evaluated only by the inline const array below.
    const fn new() -> Self {
        Self {
            pending: AtomicBool::new(false),
            waiters: RawMutex::new(SlotList::new()),
        }
    }
}
