// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use super::mode::{Async, Mode, Sync};
use super::wait_queue::{WaitQueue, Waiter, block_on};
use crate::telemetry::{self, EventKind};

/// A reusable barrier with a compile-time synchronization strategy.
///
/// The default [`Sync`] mode uses a native blocking barrier. Select [`Async`]
/// to additionally support executor-independent asynchronous waits.
#[derive(Debug)]
pub struct Barrier<M: Mode = Sync> {
    raw: M::BarrierState,
}

/// Internal native blocking barrier state.
#[doc(hidden)]
#[derive(Debug)]
pub struct StateSync {
    phase: std::sync::Mutex<Phase>,
    changed: std::sync::Condvar,
    parties: u32,
}

#[derive(Debug)]
struct Phase {
    generation: u64,
    arrived: u32,
}

impl StateSync {
    pub(super) const fn new(parties: u32) -> Self {
        Self {
            phase: std::sync::Mutex::new(Phase { generation: 0, arrived: 0 }),
            changed: std::sync::Condvar::new(),
            parties,
        }
    }
}

/// Internal asynchronous barrier state.
#[doc(hidden)]
#[derive(Debug)]
pub struct StateAsync {
    parties: u32,
    state: AtomicU64,
    waiters: WaitQueue,
}

impl<M: Mode> Barrier<M> {
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
            raw: M::barrier_new(parties),
        }
    }

    fn record(&self, kind: EventKind) {
        telemetry::record(kind, std::ptr::from_ref(self).cast::<()>());
    }
}

impl StateAsync {
    pub(super) const fn new(parties: u32) -> Self {
        Self {
            parties,
            state: AtomicU64::new(0),
            waiters: WaitQueue::new(),
        }
    }
}

impl Barrier<Sync> {
    /// Blocks until all participants reach this barrier generation.
    ///
    /// # Panics
    ///
    /// Panics if a previous internal panic poisoned the barrier state.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Mutating barrier progress conditions turns tests into unbounded scheduler waits.
    pub fn wait(&self) -> BarrierWaitResult {
        // std::sync::Barrier only identifies the final arrival after waiting.
        // A native mutex/condvar pair lets telemetry mark actual contention
        // before blocking, rather than incorrectly measuring only its return.
        let mut phase = self.raw.phase.lock().expect("barrier state is never held across user code");
        let generation = phase.generation;
        phase.arrived += 1;
        let leader = phase.arrived == self.raw.parties;
        if leader {
            phase.arrived = 0;
            phase.generation = generation.wrapping_add(1);
            self.record(EventKind::BarrierAccess);
            self.record(EventKind::BarrierRelease);
            self.raw.changed.notify_all();
        } else {
            self.record(EventKind::BarrierContention);
            while phase.generation == generation {
                phase = self.raw.changed.wait(phase).expect("barrier state is never held across user code");
            }
            self.record(EventKind::BarrierAccess);
        }
        BarrierWaitResult { leader }
    }
}

impl Barrier<Async> {
    /// Returns a future that waits for all participants to reach the barrier.
    pub fn wait_async(&self) -> BarrierWait<'_> {
        BarrierWait {
            barrier: self,
            generation: None,
            waiter: None,
            completed: false,
        }
    }

    /// Blocks the current thread until all participants reach the barrier.
    pub fn wait(&self) -> BarrierWaitResult {
        block_on(self.wait_async())
    }

    fn generation(&self) -> u32 {
        (self.raw.state.load(Ordering::Acquire) >> 32) as u32
    }

    #[expect(clippy::cast_possible_truncation, reason = "the low 32 state bits contain the participant count")]
    const fn count(state: u64) -> u32 {
        state as u32
    }

    fn arrive(&self) -> Arrival {
        let state = self
            .raw
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                let generation = (state >> 32) as u32;
                let count = Self::count(state);
                Some(if count + 1 == self.raw.parties {
                    u64::from(generation.wrapping_add(1)) << 32
                } else {
                    state + 1
                })
            })
            .expect("barrier arrival always supplies a next state");
        let generation = (state >> 32) as u32;
        let count = Self::count(state);
        if count + 1 == self.raw.parties {
            self.record(EventKind::BarrierAccess);
            self.record(EventKind::BarrierRelease);
            self.raw.waiters.wake_all();
            Arrival::Leader
        } else {
            self.record(EventKind::BarrierContention);
            Arrival::Waiting(generation)
        }
    }

    fn cancel(&self, generation: u32) -> bool {
        self.raw
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                if (state >> 32) as u32 != generation {
                    return None;
                }
                let count = Self::count(state);
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

/// A future returned by [`Barrier::wait_async`].
#[derive(Debug)]
#[must_use = "futures do nothing unless polled or awaited"]
pub struct BarrierWait<'a> {
    barrier: &'a Barrier<Async>,
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

        self.poll_registered(cx, generation)
    }
}

impl BarrierWait<'_> {
    fn poll_registered(&mut self, cx: &Context<'_>, generation: u32) -> Poll<BarrierWaitResult> {
        let barrier = self.barrier;
        let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
        waiter.register(cx.waker());
        if barrier
            .raw
            .waiters
            .enqueue_if_needed(&waiter, || barrier.generation() != generation)
        {
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
            self.barrier.raw.waiters.cancel(waiter);
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

#[cfg(test)]
mod tests {
    use std::task::Waker;

    use super::*;

    #[test]
    fn generation_change_during_waiter_registration_completes_wait() {
        let barrier = Barrier::<Async>::new(2);
        let mut wait = barrier.wait_async();
        let context = Context::from_waker(Waker::noop());

        assert_eq!(wait.poll_registered(&context, 1), Poll::Ready(BarrierWaitResult { leader: false }));
    }

    #[test]
    fn generation_reads_the_upper_state_bits() {
        let barrier = Barrier::<Async>::new(3);
        barrier.raw.state.store((7_u64 << 32) | 2, Ordering::Relaxed);

        assert_eq!(barrier.generation(), 7);
    }

    #[test]
    fn arrival_advances_count_without_changing_generation() {
        let barrier = Barrier::<Async>::new(3);
        barrier.raw.state.store((7_u64 << 32) | 1, Ordering::Relaxed);

        assert!(matches!(barrier.arrive(), Arrival::Waiting(7)));
        assert_eq!(barrier.raw.state.load(Ordering::Relaxed), (7_u64 << 32) | 2);
    }

    #[test]
    fn final_arrival_advances_generation_and_resets_count() {
        let barrier = Barrier::<Async>::new(3);
        barrier.raw.state.store((7_u64 << 32) | 2, Ordering::Relaxed);

        assert!(matches!(barrier.arrive(), Arrival::Leader));
        assert_eq!(barrier.raw.state.load(Ordering::Relaxed), 8_u64 << 32);
    }

    #[test]
    fn cancellation_withdraws_only_from_the_observed_generation() {
        let barrier = Barrier::<Async>::new(3);
        barrier.raw.state.store((7_u64 << 32) | 2, Ordering::Relaxed);

        assert!(!barrier.cancel(6));
        assert_eq!(barrier.raw.state.load(Ordering::Relaxed), (7_u64 << 32) | 2);
        assert!(barrier.cancel(7));
        assert_eq!(barrier.raw.state.load(Ordering::Relaxed), (7_u64 << 32) | 1);
    }
}
