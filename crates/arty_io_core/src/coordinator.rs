// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::task::{Wake, Waker};

use performables::arc::Arc;
use performables::sync::condition::Condvar;
use performables::sync::mutex::{Mutex, MutexGuard};

/// Coordinates wait interruption and outstanding work for one runtime worker.
///
/// The runtime owns one coordinator. Drivers interact with it only through
/// [`Cycle`](crate::Cycle), which creates non-cloneable [`PendingWork`] values and a stable
/// interruption waker.
///
/// A driver creates one [`PendingWork`] value for each unit of work that can outlive
/// `execute_cycle`. After the primary returns, the runtime interrupts remaining waits and blocks
/// until all pending work is
/// completed or dropped.
///
/// The coordinator is intentionally not cloneable.
pub struct Coordinator {
    inner: Arc<Inner>,
}

struct Inner {
    // Coordination is a hot, mostly uncontended path. The workspace performables primitives keep
    // uncontended acquisition allocation-free and retain synchronization telemetry.
    state: Mutex<State>,
    completed: Condvar,
}

#[derive(Default)]
struct State {
    interrupted: bool,
    waiting: bool,
    wakers: Vec<RegisteredWaker>,
    pending_work: usize,
    next_work_id: usize,
}

struct RegisteredWaker {
    work_id: Option<usize>,
    waker: Waker,
}

impl Coordinator {
    /// Creates an idle coordinator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                completed: Condvar::new(),
            }),
        }
    }

    /// Begins a new logical cycle, clearing interruption and prior waker registrations.
    ///
    /// The runtime calls this exactly once before checking work for the next cycle. It first
    /// completes any previous cycle, then clears its interruption state and registrations.
    /// Wakers already being dispatched may finish after this call, but cannot replace
    /// registrations added for the new cycle.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "only the runtime's exclusive coordinator owner may begin a cycle"
    )]
    #[inline]
    pub fn begin_cycle(&mut self) {
        let mut state = self.inner.complete_cycle();
        // Interruption already drained the registrations. Keep the empty allocation for reuse.
        state.interrupted = false;
        state.next_work_id = 0;
    }

    /// Returns a stable waker that interrupts the current cycle.
    ///
    /// Runtime task wakers may retain this value across cycles.
    #[must_use]
    #[inline]
    pub fn interrupt_waker(&self) -> Waker {
        Waker::from(Arc::clone(&self.inner))
    }

    /// Ends the current cycle.
    ///
    /// This first interrupts all registered waits, then blocks until every outstanding
    /// [`PendingWork`] has completed or been dropped.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "only the runtime's exclusive coordinator owner may complete a cycle"
    )]
    #[inline]
    pub fn complete_cycle(&mut self) {
        drop(self.inner.complete_cycle());
    }

    pub(crate) fn start_work(&self) -> PendingWork {
        let work_id = {
            let mut state = self.inner.lock_state();
            state.pending_work = state.pending_work.saturating_add(1);
            let work_id = state.next_work_id;
            state.next_work_id = state.next_work_id.wrapping_add(1);
            work_id
        };
        PendingWork {
            inner: Arc::clone(&self.inner),
            work_id,
            active: true,
        }
    }
}

impl Default for Coordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Coordinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (interrupted, pending_work) = {
            let state = self.inner.lock_state();
            (state.interrupted, state.pending_work)
        };
        f.debug_struct("Coordinator")
            .field("interrupted", &interrupted)
            .field("pending_work", &pending_work)
            .finish_non_exhaustive()
    }
}

impl Inner {
    fn lock_state(&self) -> MutexGuard<'_, State> {
        match self.state.lock_sync_result() {
            Ok(state) => state,
            Err(error) => {
                let state = error.into_inner();
                self.state.clear_poison();
                state
            }
        }
    }

    fn on_interrupt(&self, work_id: usize, waker: Waker) {
        let mut state = self.lock_state();
        if state.interrupted {
            drop(state);
            waker.wake();
        } else {
            state.wakers.push(RegisteredWaker {
                work_id: Some(work_id),
                waker,
            });
        }
    }

    #[inline]
    fn complete_cycle(&self) -> MutexGuard<'_, State> {
        let state = self.interrupt(self.lock_state());
        if state.pending_work == 0 {
            return state;
        }
        self.wait_for_work(state)
    }

    fn wait_for_work<'a>(&'a self, mut state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        // The exclusive coordinator owner is the only possible completion waiter.
        state.waiting = true;
        while state.pending_work != 0 {
            state = self.completed.wait_sync(state);
        }
        state.waiting = false;
        state
    }

    #[inline]
    fn interrupt<'a>(&'a self, mut state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        if state.interrupted {
            return state;
        }
        state.interrupted = true;
        if state.wakers.is_empty() {
            return state;
        }
        self.dispatch_interrupt(state)
    }

    fn dispatch_interrupt<'a>(&'a self, mut state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        let mut wakers = std::mem::take(&mut state.wakers);
        drop(state);

        while let Some(registered) = wakers.pop() {
            if registered.work_id.is_some() {
                registered.waker.wake();
            }
        }

        let mut state = self.lock_state();
        if state.wakers.is_empty() && state.wakers.capacity() < wakers.capacity() {
            std::mem::swap(&mut state.wakers, &mut wakers);
        }
        state
    }

    fn complete_work(&self, work_id: usize, interrupt: bool) {
        let mut state = self.lock_state();
        for registered in &mut state.wakers {
            if registered.work_id == Some(work_id) {
                registered.work_id = None;
            }
        }
        if interrupt {
            state = self.interrupt(state);
        }
        state.pending_work = state.pending_work.saturating_sub(1);
        if state.pending_work == 0 && state.waiting {
            self.completed.notify_all();
        }
    }
}

impl Wake for Inner {
    fn wake(self: std::sync::Arc<Self>) {
        drop(self.interrupt(self.lock_state()));
    }

    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        drop(self.interrupt(self.lock_state()));
    }
}

/// Completion ownership for one unit of work in one runtime cycle.
///
/// The value is intentionally not cloneable. Move it to work that can outlive `execute_cycle`.
/// After publishing work, call [`complete`](Self::complete) to interrupt other waits and
/// release the completion barrier. If no work was published, drop the value; dropping releases
/// the barrier without interrupting the cycle.
pub struct PendingWork {
    inner: Arc<Inner>,
    work_id: usize,
    active: bool,
}

impl PendingWork {
    /// Invokes `waker` when this cycle is interrupted.
    ///
    /// If interruption was already requested, `waker` is invoked before this method returns.
    /// Calling this method repeatedly may cause redundant wake calls, which the native waker must
    /// safely coalesce.
    ///
    /// The waker can be invoked inline on any thread that completes work or ends a cycle. It must
    /// only signal its native wait and return promptly. It must not synchronously wait for this
    /// coordinator, join another task, or acquire a lock still held by the completing work.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "registration is an exclusive operation on this pending-work capability"
    )]
    #[inline]
    pub fn on_interrupt(&mut self, waker: Waker) {
        self.inner.on_interrupt(self.work_id, waker);
    }

    /// Returns whether this cycle was interrupted.
    #[must_use]
    #[inline]
    pub fn is_interrupted(&self) -> bool {
        self.inner.lock_state().interrupted
    }

    /// Reports that this work completed with work ready for the owning driver.
    ///
    /// Call this only after making the work visible. It interrupts other registered waits, then
    /// releases the runtime's completion barrier. Drop the value instead when the work ends
    /// without publishing anything.
    #[inline]
    pub fn complete(mut self) {
        self.mark_completed(true);
    }

    #[inline]
    fn mark_completed(&mut self, interrupt: bool) {
        if !self.active {
            return;
        }
        self.active = false;
        self.inner.complete_work(self.work_id, interrupt);
    }
}

impl Drop for PendingWork {
    fn drop(&mut self) {
        self.mark_completed(false);
    }
}

impl fmt::Debug for PendingWork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingWork")
            .field("work_id", &self.work_id)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn poisoned_state_is_recovered_and_cleared() {
        let mut coordinator = Coordinator::new();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _state = coordinator.inner.state.lock_sync();
            panic!("poison coordinator state");
        }));
        assert!(result.is_err());
        assert!(coordinator.inner.state.is_poisoned());

        coordinator.begin_cycle();

        assert!(!coordinator.inner.state.is_poisoned());
    }

    fn releases_waiting_owner(publish: bool) {
        let mut coordinator = Coordinator::new();
        coordinator.begin_cycle();
        let first = coordinator.start_work();
        let last = coordinator.start_work();
        let inner = Arc::clone(&coordinator.inner);
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            coordinator.complete_cycle();
            done_tx.send(coordinator).unwrap();
        });

        let deadline = Instant::now() + Duration::from_secs(10);
        while !inner.lock_state().waiting {
            assert!(Instant::now() < deadline, "the owner must enter the completion wait");
            thread::yield_now();
        }
        drop(first);
        let waiting_for_last = inner.lock_state().waiting;
        if publish {
            last.complete();
        } else {
            drop(last);
        }
        let mut coordinator = done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        worker.join().unwrap();
        coordinator.begin_cycle();

        let state = inner.lock_state();
        assert_eq!(
            (waiting_for_last, state.waiting, state.pending_work, state.interrupted),
            (true, false, 0, false)
        );
    }

    #[test]
    fn last_drop_notifies_a_waiting_owner() {
        releases_waiting_owner(false);
    }

    #[test]
    fn last_completion_notifies_a_waiting_owner() {
        releases_waiting_owner(true);
    }
}
