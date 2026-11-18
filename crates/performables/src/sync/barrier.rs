// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, MutexGuard as PhaseGuard};
use std::task::{Context, Poll};

use super::wait_queue::{WaitQueue, Waiter, block_on};
use super::{Async, Mode, Sync};
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
    pub fn wait(&self) -> BarrierWaitOutcome {
        self.wait_with(|phase| self.raw.changed.wait(phase).expect("barrier state is never held across user code"))
    }

    fn wait_with<'a>(&'a self, mut wait: impl FnMut(PhaseGuard<'a, Phase>) -> PhaseGuard<'a, Phase>) -> BarrierWaitOutcome {
        if self.raw.parties == 1 {
            self.record(EventKind::BarrierAccess);
            self.record(EventKind::BarrierRelease);
            return BarrierWaitOutcome { leader: true };
        }
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
                // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "Windows CI mutant fc57783cdb5b: removing the wait retains the phase lock and exceeded the test timeout")]
                phase = wait(phase);
            }
            self.record(EventKind::BarrierAccess);
        }
        BarrierWaitOutcome { leader }
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
    ///
    /// This method must not run on an executor thread required to make progress
    /// on another participant. Inside an async task, await [`Self::wait_async`]
    /// instead so the executor can run the remaining participants.
    pub fn wait(&self) -> BarrierWaitOutcome {
        match self.arrive() {
            Arrival::Leader => BarrierWaitOutcome { leader: true },
            // Arrival is already counted; the future must retain its generation.
            Arrival::Waiting(generation) => block_on(BarrierWait {
                barrier: self,
                generation: Some(generation),
                waiter: None,
                completed: false,
            }),
        }
    }

    fn generation(&self) -> u32 {
        (self.raw.state.load(Ordering::Acquire) >> 32) as u32
    }

    #[expect(clippy::cast_possible_truncation, reason = "the low 32 state bits contain the participant count")]
    const fn count(state: u64) -> u32 {
        state as u32
    }

    fn arrive(&self) -> Arrival {
        // With one participant no other arrival can require publication or a wake.
        if self.raw.parties == 1 {
            self.record(EventKind::BarrierAccess);
            self.record(EventKind::BarrierRelease);
            return Arrival::Leader;
        }
        let state = self
            .raw
            .state
            .try_update(Ordering::AcqRel, Ordering::Acquire, |state| {
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
            .try_update(Ordering::AcqRel, Ordering::Acquire, |state| {
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
    type Output = BarrierWaitOutcome;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.generation.is_none() {
            match self.barrier.arrive() {
                Arrival::Leader => {
                    self.completed = true;
                    return Poll::Ready(BarrierWaitOutcome { leader: true });
                }
                Arrival::Waiting(generation) => self.generation = Some(generation),
            }
        }

        let generation = self.generation.expect("arrival records a generation");
        if self.barrier.generation() != generation {
            self.completed = true;
            self.barrier.record(EventKind::BarrierAccess);
            return Poll::Ready(BarrierWaitOutcome { leader: false });
        }

        self.poll_registered(cx, generation)
    }
}

impl BarrierWait<'_> {
    fn poll_registered(&mut self, cx: &Context<'_>, generation: u32) -> Poll<BarrierWaitOutcome> {
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
            Poll::Ready(BarrierWaitOutcome { leader: false })
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

/// Outcome of a completed barrier generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BarrierWaitOutcome {
    leader: bool,
}

impl BarrierWaitOutcome {
    /// Returns whether this participant released the barrier.
    #[must_use]
    pub const fn is_leader(self) -> bool {
        self.leader
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn nov4_blocking_async_barrier_cancels_the_arrival_when_its_injected_parker_unwinds() {
        let barrier = super::Barrier::<crate::sync::Async>::new(2);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::sync::effects::with_park(
                || panic!("injected parker failure"),
                || {
                    barrier.wait();
                },
            );
        }));
        let panic = outcome.unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"injected parker failure"));
        assert_eq!(
            super::Barrier::<crate::sync::Async>::count(barrier.raw.state.load(std::sync::atomic::Ordering::Acquire)),
            0
        );
        let mut next = std::pin::pin!(barrier.wait_async());
        assert!(
            next.as_mut()
                .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
                .is_pending()
        );
    }
    #[test]
    #[cfg(feature = "seismograph")]
    fn gamma_round2_native_barrier_tracks_each_phase_transition_and_completion_without_parking() {
        use seismograph::recorder::event::EventKind as RecordedKind;

        crate::gamma_round2_telemetry::enable_recording();
        for leader in [false, true] {
            let barrier = super::Barrier::<Sync>::new(2);
            if leader {
                barrier.raw.phase.lock().unwrap().arrived = 1;
            }
            let accesses = crate::gamma_round2_telemetry::recorded_test_events(RecordedKind::BarrierAccess);
            let releases = crate::gamma_round2_telemetry::recorded_test_events(RecordedKind::BarrierRelease);
            let contentions = crate::gamma_round2_telemetry::recorded_test_events(RecordedKind::BarrierContention);
            let mut waits = 0;
            let outcome = barrier.wait_with(|mut phase| {
                waits += 1;
                phase.arrived = 0;
                phase.generation = phase.generation.wrapping_add(1);
                phase
            });
            assert_eq!(outcome.is_leader(), leader);
            assert_eq!(waits, usize::from(!leader));
            let phase = barrier.raw.phase.lock().unwrap();
            assert_eq!(phase.arrived, 0);
            assert_eq!(phase.generation, 1);
            drop(phase);
            assert_eq!(
                crate::gamma_round2_telemetry::recorded_test_events(RecordedKind::BarrierAccess),
                accesses + 1
            );
            assert_eq!(
                crate::gamma_round2_telemetry::recorded_test_events(RecordedKind::BarrierRelease),
                releases + usize::from(leader)
            );
            assert_eq!(
                crate::gamma_round2_telemetry::recorded_test_events(RecordedKind::BarrierContention),
                contentions + usize::from(!leader)
            );
        }
    }

    use std::pin::pin;
    use std::task::Waker;
    use std::time::Duration;

    use super::*;

    #[test]
    fn gamma_round2_registration_completion_marks_the_barrier_wait_terminal() {
        let barrier = Barrier::<Async>::new(2);
        let mut wait = barrier.wait_async();
        let context = Context::from_waker(Waker::noop());
        assert!(Pin::new(&mut wait).poll(&mut Context::from_waker(Waker::noop())).is_pending());
        let generation = wait.generation.unwrap();
        assert!(barrier.wait().is_leader());
        assert_eq!(
            wait.poll_registered(&context, generation),
            Poll::Ready(BarrierWaitOutcome { leader: false })
        );
        assert!(wait.completed);
        assert!(wait.waiter.is_none());
    }

    #[test]
    fn gamma_round2_single_party_arrival_does_not_change_the_packed_state() {
        let barrier = Barrier::<Async>::new(1);
        let before = barrier.raw.state.load(Ordering::Acquire);
        assert!(barrier.wait().is_leader());
        assert_eq!(barrier.raw.state.load(Ordering::Acquire), before);
    }

    #[test]
    fn gamma_survivor_barrier_terminal_metadata_and_native_generation_wrap_are_exact() {
        let single = Barrier::<Sync>::new(1);
        assert!(single.wait().is_leader());
        assert_eq!(single.raw.phase.lock().unwrap().generation, 0);
        let native = Barrier::<Sync>::new(2);
        {
            let mut phase = native.raw.phase.lock().unwrap();
            phase.arrived = 1;
            phase.generation = u64::MAX;
        }
        assert!(native.wait().is_leader());
        {
            let phase = native.raw.phase.lock().unwrap();
            assert_eq!(phase.arrived, 0);
            assert_eq!(phase.generation, 0);
        }
        let asynchronous = Barrier::<Async>::new(2);
        let mut first = asynchronous.wait_async();
        let mut last = asynchronous.wait_async();
        let mut cx = Context::from_waker(Waker::noop());
        assert!(Pin::new(&mut first).poll(&mut cx).is_pending());
        assert!(first.waiter.is_some());
        assert!(Pin::new(&mut last).poll(&mut cx).is_ready());
        assert!(last.completed);
        assert!(Pin::new(&mut first).poll(&mut cx).is_ready());
        assert!(first.completed);
        assert!(first.waiter.is_some());
        let racing = Barrier::<Async>::new(2);
        let mut follower = racing.wait_async();
        assert!(Pin::new(&mut follower).poll(&mut cx).is_pending());
        let generation = follower.generation.unwrap();
        assert!(Pin::new(&mut racing.wait_async()).poll(&mut cx).is_ready());
        assert!(follower.poll_registered(&cx, generation).is_ready());
        assert!(follower.waiter.is_none());
    }

    #[test]
    fn generation_change_during_waiter_registration_completes_wait() {
        let barrier = Barrier::<Async>::new(2);
        let mut wait = barrier.wait_async();
        let context = Context::from_waker(Waker::noop());

        assert_eq!(wait.poll_registered(&context, 1), Poll::Ready(BarrierWaitOutcome { leader: false }));
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
    fn blocking_leader_releases_async_waiters_once_per_generation() {
        let barrier = Barrier::<Async>::new(2);
        let mut context = Context::from_waker(Waker::noop());
        for generation in 1..=3 {
            let mut wait = pin!(barrier.wait_async());
            assert!(wait.as_mut().poll(&mut context).is_pending());
            assert!(barrier.wait().is_leader());
            assert_eq!(wait.as_mut().poll(&mut context), Poll::Ready(BarrierWaitOutcome { leader: false }));
            assert_eq!(barrier.raw.state.load(Ordering::Relaxed), generation << 32);
        }
    }

    #[test]
    fn blocking_waiter_retains_its_first_arrival() {
        let barrier = Arc::new(Barrier::<Async>::new(2));
        let worker = std::thread::spawn({
            let barrier = Arc::clone(&barrier);
            move || barrier.wait()
        });
        let deadline = (!testing_aids::is_mutation_testing())
            .then(|| std::time::Instant::now() + Duration::from_secs(if cfg!(miri) { 120 } else { 2 }));
        while Barrier::<Async>::count(barrier.raw.state.load(Ordering::Acquire)) == 0 {
            if let Some(deadline) = deadline {
                assert!(
                    std::time::Instant::now() < deadline,
                    "blocking participant must arrive before the deadline"
                );
            }
            std::thread::yield_now();
        }
        assert!(block_on(barrier.wait_async()).is_leader());
        assert!(!worker.join().unwrap().is_leader());
        assert_eq!(barrier.raw.state.load(Ordering::Relaxed), 1_u64 << 32);
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
