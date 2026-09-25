// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::task::Waker;

/// A stable worker notification handle with one-shot interruption between resets.
///
/// Drivers register independently owned native-wait wakers. The first request wakes every
/// registration; further requests have no effect until reset. Late registrations wake immediately.
/// Wake calls run outside the internal lock and may register or request interruption reentrantly.
///
/// Interruption ends waiting, not I/O operations. Callbacks must be bounded and non-blocking.
/// They must not request another interruption merely because an interrupt was observed, or they
/// can cause a wake-up loop. A native interrupt must be latched so registration before entering
/// the wait cannot lose a wake-up.
///
/// Publish real work before calling [`request`](Self::request). All clones obtained through
/// [`Cycle::interruptor`](crate::Cycle::interruptor)
/// refer to the same worker notification state throughout its lifetime. Retained observer handles
/// continue notifying the current cycle after the runtime resets.
///
/// Wakers must remain safe after a driver is dropped. Dropping a handle or finishing a cycle
/// does not request interruption: an unconditional end-of-cycle wake could latch an interruption
/// for the next wait and cause an idle spin. Retained clones remain usable.
#[derive(Clone, Default)]
pub struct Interruptor {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    requested: bool,
    wakers: Vec<Waker>,
}

impl Interruptor {
    /// Creates an uninterrupted worker notification handle.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Begins a new interruption round on the shared state, reusing waker storage.
    ///
    /// All existing clones observe the reset and target the new round. Pending registrations
    /// from the previous round are discarded without waking them. Callbacks already being
    /// dispatched may still finish; their completion cannot overwrite new registrations.
    ///
    /// Only the owning runtime resets, exactly once at the start of a logical cycle, before
    /// checking task, control, and driver queues. It must not reset between driver invocations or
    /// for a registration initialization cycle. Work published before reset is found by those
    /// checks; a request after reset remains latched for the new round. Resetting after the final
    /// work check could lose a notification. Drivers register their native-wait wakers anew.
    ///
    /// Discarded wakers are dropped outside the lock. Reset is not a quiescence barrier and
    /// must not depend on outstanding native waiters or broadcasts already having returned.
    ///
    /// # Panics
    ///
    /// Panics if internal bookkeeping was poisoned by an earlier panic.
    #[inline]
    pub fn reset(&self) {
        let mut wakers = {
            let mut state = self.state.lock().expect("interruptor bookkeeping must not be poisoned");
            state.requested = false;
            std::mem::take(&mut state.wakers)
        };
        wakers.clear();
        self.recycle(wakers);
    }

    /// Registers an owned native-wait callback, or wakes it immediately if already interrupted.
    ///
    /// Each live registration is notified once by a request; reset discards pending
    /// registrations. Register once per native wait per cycle. Redundant registrations may
    /// cause redundant calls, which the native waker must safely coalesce.
    ///
    /// # Panics
    ///
    /// Panics if internal bookkeeping was poisoned, or calling [`Waker::wake`] panics.
    #[inline]
    pub fn register(&self, waker: Waker) {
        let mut state = self.state.lock().expect("interruptor bookkeeping must not be poisoned");
        if !state.requested {
            state.wakers.push(waker);
            return;
        }
        drop(state);
        waker.wake();
    }

    /// Requests interruption once for the current round, regardless of when this handle was cloned.
    ///
    /// Concurrent registration either joins this broadcast or observes the request and wakes
    /// immediately. A waker panic is a programming error; completion of the remaining wake calls
    /// is not guaranteed during unwinding.
    ///
    /// This is notification, not a barrier that waits for native waiters to return.
    ///
    /// # Panics
    ///
    /// Panics if internal bookkeeping was poisoned or calling [`Waker::wake`] panics.
    #[inline]
    pub fn request(&self) {
        let mut wakers = {
            let mut state = self.state.lock().expect("interruptor bookkeeping must not be poisoned");
            if state.requested {
                return;
            }
            state.requested = true;
            std::mem::take(&mut state.wakers)
        };
        while let Some(waker) = wakers.pop() {
            waker.wake();
        }
        self.recycle(wakers);
    }

    fn recycle(&self, mut wakers: Vec<Waker>) {
        let mut state = self.state.lock().expect("interruptor bookkeeping must not be poisoned");
        // A previous broadcast or a callback's destructor may race reset and new registrations.
        if state.wakers.is_empty() && state.wakers.capacity() < wakers.capacity() {
            std::mem::swap(&mut state.wakers, &mut wakers);
        }
        drop(state);
    }

    /// Returns whether interruption has been requested.
    ///
    /// A false result is only a snapshot. The registered native waker must still close the race
    /// between this check and entering a blocking wait.
    ///
    /// # Panics
    ///
    /// Panics if internal bookkeeping was poisoned by an earlier panic.
    #[must_use]
    #[inline]
    pub fn is_requested(&self) -> bool {
        self.state.lock().expect("interruptor bookkeeping must not be poisoned").requested
    }
}

impl fmt::Debug for Interruptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Interruptor")
            .field("requested", &self.is_requested())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn shared_reset_reuses_storage_after_broadcast_and_discards_old_callbacks() {
        let interruptor = Interruptor::new();
        let retained = interruptor.clone();
        for _ in 0..8 {
            interruptor.register(Waker::noop().clone());
        }
        let allocation = Arc::as_ptr(&interruptor.state);
        let capacity = interruptor.state.lock().unwrap().wakers.capacity();
        interruptor.request();
        interruptor.reset();
        assert_eq!(Arc::as_ptr(&interruptor.state), allocation);
        assert!(Arc::ptr_eq(&interruptor.state, &retained.state));
        assert_eq!(interruptor.state.lock().unwrap().wakers.capacity(), capacity);
        assert!(!interruptor.is_requested());
        assert!(!retained.is_requested());
        assert!(interruptor.state.lock().unwrap().wakers.is_empty());

        interruptor.register(Waker::noop().clone());
        interruptor.reset();
        assert_eq!(Arc::as_ptr(&interruptor.state), allocation);
        assert!(interruptor.state.lock().unwrap().wakers.is_empty());
    }
}
