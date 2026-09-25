// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::task::{Wake, Waker};

/// Coordinates wait interruption and outstanding work for one runtime worker.
///
/// The runtime owns one coordinator. Drivers interact with it only through
/// [`Cycle`](crate::Cycle), which creates non-cloneable [`CoordinationToken`] values and a stable
/// interruption waker.
///
/// A driver creates one token for each unit of work that can outlive `execute_cycle`. After the
/// primary returns, the runtime interrupts remaining waits and blocks until all tokens are
/// completed or dropped.
///
/// The coordinator is intentionally not cloneable.
pub struct Coordinator {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    completed: Condvar,
}

#[derive(Default)]
struct State {
    interrupted: bool,
    wakers: Vec<Waker>,
    pending_work: usize,
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
    /// The runtime calls this exactly once before checking work for the next cycle and only after
    /// [`complete_cycle`](Self::complete_cycle) returned for the previous one. Wakers already
    /// being dispatched may finish after this call, but cannot replace registrations added for
    /// the new cycle.
    ///
    /// # Panics
    ///
    /// Panics if work from the previous cycle is still pending or bookkeeping was poisoned.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "only the runtime's exclusive coordinator owner may begin a cycle"
    )]
    pub fn begin_cycle(&mut self) {
        let pending_work = self
            .inner
            .state
            .lock()
            .expect("coordinator bookkeeping must not be poisoned")
            .pending_work;
        assert_eq!(pending_work, 0, "all coordination tokens must complete before beginning a cycle");
        let mut wakers = {
            let mut state = self.inner.state.lock().expect("coordinator bookkeeping must not be poisoned");
            state.interrupted = false;
            std::mem::take(&mut state.wakers)
        };
        wakers.clear();
        self.inner.recycle(wakers);
    }

    /// Interrupts every native wait registered for the current cycle.
    ///
    /// Repeated calls coalesce until the next [`begin_cycle`](Self::begin_cycle).
    ///
    /// # Panics
    ///
    /// Panics if bookkeeping was poisoned or calling a registered [`Waker::wake`] panics.
    pub fn interrupt(&self) {
        self.inner.interrupt();
    }

    /// Returns a stable waker that interrupts the current cycle.
    ///
    /// Runtime task wakers may retain this value across cycles.
    #[must_use]
    pub fn interrupt_waker(&self) -> Waker {
        Waker::from(Arc::clone(&self.inner))
    }

    /// Ends the current cycle.
    ///
    /// This first interrupts all registered waits, then blocks until every outstanding
    /// [`CoordinationToken`] has completed or been dropped.
    ///
    /// # Panics
    ///
    /// Panics if bookkeeping was poisoned or calling a registered [`Waker::wake`] panics.
    pub fn complete_cycle(&self) {
        self.interrupt();
        self.wait_for_idle();
    }

    /// Blocks until no coordination tokens remain.
    ///
    /// This does not interrupt registered waits. The runtime uses it for a zero-wait
    /// initialization pass that must finish before a context is published.
    ///
    /// # Panics
    ///
    /// Panics if bookkeeping was poisoned.
    pub fn wait_for_idle(&self) {
        let mut state = self.inner.state.lock().expect("coordinator bookkeeping must not be poisoned");
        while state.pending_work != 0 {
            state = self
                .inner
                .completed
                .wait(state)
                .expect("coordinator bookkeeping must not be poisoned");
        }
    }

    pub(crate) fn start_work(&self) -> CoordinationToken {
        {
            let mut state = self.inner.state.lock().expect("coordinator bookkeeping must not be poisoned");
            state.pending_work = state.pending_work.checked_add(1).expect("coordinator work count must not overflow");
        }
        CoordinationToken {
            inner: Arc::clone(&self.inner),
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
        let state = self.inner.state.lock().expect("coordinator bookkeeping must not be poisoned");
        f.debug_struct("Coordinator")
            .field("interrupted", &state.interrupted)
            .field("pending_work", &state.pending_work)
            .finish_non_exhaustive()
    }
}

impl Inner {
    fn on_interrupted(&self, waker: Waker) {
        let mut state = self.state.lock().expect("coordinator bookkeeping must not be poisoned");
        if state.interrupted {
            drop(state);
            waker.wake();
        } else {
            state.wakers.push(waker);
        }
    }

    fn interrupt(&self) {
        let mut wakers = {
            let mut state = self.state.lock().expect("coordinator bookkeeping must not be poisoned");
            if state.interrupted {
                return;
            }
            state.interrupted = true;
            std::mem::take(&mut state.wakers)
        };
        while let Some(waker) = wakers.pop() {
            waker.wake();
        }
        self.recycle(wakers);
    }

    fn recycle(&self, mut wakers: Vec<Waker>) {
        let mut state = self.state.lock().expect("coordinator bookkeeping must not be poisoned");
        if state.wakers.is_empty() && state.wakers.capacity() < wakers.capacity() {
            std::mem::swap(&mut state.wakers, &mut wakers);
        }
    }
}

impl Wake for Inner {
    fn wake(self: Arc<Self>) {
        self.interrupt();
    }
}

/// Completion ownership for one unit of work in one runtime cycle.
///
/// The token is intentionally not cloneable. Move it to work that can outlive `execute_cycle`.
/// After publishing work, call [`work_ready`](Self::work_ready) to interrupt other waits and
/// release the completion barrier. If no work was published, drop the token; dropping releases
/// the barrier without interrupting the cycle.
pub struct CoordinationToken {
    inner: Arc<Inner>,
    active: bool,
}

impl CoordinationToken {
    /// Invokes `waker` when this cycle is interrupted.
    ///
    /// If interruption was already requested, `waker` is invoked before this method returns.
    /// Calling this method repeatedly may cause redundant wake calls, which the native waker must
    /// safely coalesce.
    ///
    /// # Panics
    ///
    /// Panics if bookkeeping was poisoned or calling [`Waker::wake`] panics.
    pub fn on_interrupted(&self, waker: Waker) {
        self.inner.on_interrupted(waker);
    }

    /// Returns whether this cycle was interrupted.
    ///
    /// # Panics
    ///
    /// Panics if bookkeeping was poisoned.
    #[must_use]
    pub fn is_interrupted(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("coordinator bookkeeping must not be poisoned")
            .interrupted
    }

    /// Reports that this work completed with work ready for the owning driver.
    ///
    /// Call this only after making the work visible. It interrupts other registered waits, then
    /// releases the runtime's completion barrier. Drop the token instead when the work ends
    /// without publishing anything.
    ///
    /// # Panics
    ///
    /// Panics if bookkeeping was poisoned, calling a registered [`Waker::wake`] panics, or the
    /// token was completed incorrectly.
    pub fn work_ready(mut self) {
        self.inner.interrupt();
        self.complete();
    }

    fn complete(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut state = self.inner.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.pending_work = state
            .pending_work
            .checked_sub(1)
            .expect("coordination token must complete exactly once");
        if state.pending_work == 0 {
            self.inner.completed.notify_all();
        }
    }
}

impl Drop for CoordinationToken {
    fn drop(&mut self) {
        self.complete();
    }
}

impl fmt::Debug for CoordinationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoordinationToken")
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}
