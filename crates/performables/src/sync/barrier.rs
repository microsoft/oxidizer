// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use super::wait_queue::{WaitQueue, Waiter, block_on};
use crate::telemetry::{self, EventKind};

const COUNT_MASK: u64 = u32::MAX as u64;

/// An executor-independent reusable barrier.
#[derive(Debug)]
pub struct Barrier {
    parties: u32,
    state: AtomicU64,
    waiters: WaitQueue,
}

impl Barrier {
    /// Creates a barrier that releases after `parties` participants arrive.
    ///
    /// # Panics
    ///
    /// Panics if `parties` is zero or exceeds [`u32::MAX`].
    #[must_use]
    pub fn new(parties: usize) -> Self {
        let parties = u32::try_from(parties).expect("barrier participant count exceeds u32::MAX");
        assert!(parties > 0, "barrier participant count must be nonzero");
        Self {
            parties,
            state: AtomicU64::new(0),
            waiters: WaitQueue::new(),
        }
    }

    /// Returns a future that waits for all participants to reach the barrier.
    pub fn wait(&self) -> BarrierWait<'_> {
        BarrierWait {
            barrier: self,
            generation: None,
            waiter: None,
            completed: false,
        }
    }

    /// Blocks the current thread until all participants reach the barrier.
    pub fn wait_sync(&self) -> BarrierWaitResult {
        block_on(self.wait())
    }

    fn record(&self, kind: EventKind) {
        telemetry::record(kind, std::ptr::from_ref(self).cast::<()>());
    }

    fn generation(&self) -> u32 {
        (self.state.load(Ordering::Acquire) >> 32) as u32
    }

    fn arrive(&self) -> Arrival {
        let state = self
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                let generation = (state >> 32) as u32;
                let count = (state & COUNT_MASK) as u32;
                Some(if count + 1 == self.parties {
                    u64::from(generation.wrapping_add(1)) << 32
                } else {
                    state + 1
                })
            })
            .expect("barrier arrival always supplies a next state");
        let generation = (state >> 32) as u32;
        let count = (state & COUNT_MASK) as u32;
        if count + 1 == self.parties {
            self.record(EventKind::BarrierAccess);
            self.record(EventKind::BarrierRelease);
            self.waiters.wake_all_marked(|| {});
            Arrival::Leader
        } else {
            self.record(EventKind::BarrierContention);
            Arrival::Waiting(generation)
        }
    }

    fn cancel(&self, generation: u32) -> bool {
        self.state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                if (state >> 32) as u32 != generation {
                    return None;
                }
                let count = (state & COUNT_MASK) as u32;
                debug_assert!(count > 0);
                Some(state - 1)
            })
            .is_ok()
    }
}

enum Arrival {
    Leader,
    Waiting(u32),
}

/// A future returned by [`Barrier::wait`].
#[derive(Debug)]
#[must_use = "futures do nothing unless polled or awaited"]
pub struct BarrierWait<'a> {
    barrier: &'a Barrier,
    generation: Option<u32>,
    waiter: Option<Arc<Waiter>>,
    completed: bool,
}

impl Future for BarrierWait<'_> {
    type Output = BarrierWaitResult;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.generation.is_none() {
            match self.barrier.arrive() {
                Arrival::Leader => {
                    self.completed = true;
                    return Poll::Ready(BarrierWaitResult { leader: true });
                }
                Arrival::Waiting(generation) => self.generation = Some(generation),
            }
        }

        let generation = self.generation.expect("arrival records a generation");
        if self.barrier.generation() != generation {
            self.completed = true;
            self.barrier.record(EventKind::BarrierAccess);
            return Poll::Ready(BarrierWaitResult { leader: false });
        }

        let barrier = self.barrier;
        let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
        waiter.register(cx.waker());
        if barrier.waiters.enqueue_if_needed(&waiter, || barrier.generation() != generation) {
            self.waiter.take();
            self.completed = true;
            barrier.record(EventKind::BarrierAccess);
            Poll::Ready(BarrierWaitResult { leader: false })
        } else {
            Poll::Pending
        }
    }
}

impl Drop for BarrierWait<'_> {
    fn drop(&mut self) {
        if let Some(waiter) = &self.waiter {
            self.barrier.waiters.cancel(waiter);
        }
        if !self.completed
            && let Some(generation) = self.generation
        {
            let _cancelled = self.barrier.cancel(generation);
        }
    }
}

/// Result returned when a barrier generation completes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BarrierWaitResult {
    leader: bool,
}

impl BarrierWaitResult {
    /// Returns whether this participant released the barrier.
    #[must_use]
    pub const fn is_leader(self) -> bool {
        self.leader
    }
}
