// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Loom model-checked tests for [`sync_thunk::StackState`].
//!
//! `StackState` is the cross-thread primitive that mediates the worker /
//! poller hand-off. It is the soundness-critical piece of `sync_thunk`: a
//! missed happens-before edge here would manifest as a use-after-free on the
//! caller's stack the moment the wrapper future is dropped.
//!
//! Loom exhaustively explores legal interleavings of the few atomic
//! operations involved. The rest of the dispatch pipeline (the crossbeam-channel
//! channel, the worker pool) is **not** loom-instrumented, so these tests
//! intentionally drive `StackState` directly rather than going through
//! `Thunker`.
//!
//! Each test models the smallest possible scenario:
//!
//! - `publish_then_drop` — worker publishes a result then signals done; the
//!   caller observes ready, takes the result, and the destructor must not
//!   release storage before the worker is fully finished.
//! - `wake_after_set` — caller sets a waker; worker wakes it. The waker must
//!   be observed at least once.
//! - `abandoned_state_drops_without_worker` — caller cancels before
//!   dispatching; destructor must not spin forever waiting for a worker that
//!   never runs.
//! - `panic_publish_then_drop` — worker marks panicked + ready + done; caller
//!   observes both flags coherently.

#![cfg(loom)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]
#![allow(clippy::std_instead_of_core, reason = "loom + std interop in tests")]
#![allow(clippy::missing_panics_doc, reason = "test code")]
#![allow(clippy::unwrap_used, reason = "test code")]

use std::task::{RawWaker, RawWakerVTable, Waker};

use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, Ordering as LoomOrdering};
use loom::thread;
use sync_thunk::StackState;

// --- Test waker that flips a loom-instrumented flag on wake -----------------

fn flag_clone(data: *const ()) -> RawWaker {
    // SAFETY: data points to a valid Arc<AtomicBool>.
    let arc = unsafe { Arc::from_raw(data.cast::<AtomicBool>()) };
    let clone = Arc::clone(&arc);
    core::mem::forget(arc);
    RawWaker::new(Arc::into_raw(clone).cast(), &FLAG_VTABLE)
}
fn flag_wake(data: *const ()) {
    // SAFETY: data points to a valid Arc<AtomicBool>.
    let arc = unsafe { Arc::from_raw(data.cast::<AtomicBool>()) };
    arc.store(true, LoomOrdering::SeqCst);
}
fn flag_wake_by_ref(data: *const ()) {
    // SAFETY: data points to a valid Arc<AtomicBool>.
    let arc = unsafe { Arc::from_raw(data.cast::<AtomicBool>()) };
    arc.store(true, LoomOrdering::SeqCst);
    core::mem::forget(arc);
}
fn flag_drop(data: *const ()) {
    // SAFETY: data points to a valid Arc<AtomicBool>.
    unsafe { drop(Arc::from_raw(data.cast::<AtomicBool>())) };
}
static FLAG_VTABLE: RawWakerVTable = RawWakerVTable::new(flag_clone, flag_wake, flag_wake_by_ref, flag_drop);

fn waker_from(flag: &Arc<AtomicBool>) -> Waker {
    let raw = Arc::into_raw(Arc::clone(flag));
    // SAFETY: vtable functions reconstruct the Arc<AtomicBool> from `data`.
    unsafe { Waker::from_raw(RawWaker::new(raw.cast(), &FLAG_VTABLE)) }
}

// --- Tests ------------------------------------------------------------------

/// The fundamental publish-and-release race.
///
/// Worker:
///   1. `complete(result)` — writes value, stores `ready = Release`.
///   2. `wake()` — drains the waker mutex and wakes (waker may be absent).
///   3. `mark_worker_done()` — `worker_done = Release`.
///
/// Caller (concurrent):
///   - Polls `is_ready()`; once ready, `take_result()` must yield the value.
///   - Drops the state at any point — the spin guard on `worker_done` must
///     prevent storage release before the worker's step 3.
///
/// Loom must find no interleaving where:
///   - `take_result()` returns `None` after `is_ready() == true`, or
///   - `drop` returns while the worker still has a borrow.
#[test]
fn publish_then_drop() {
    loom::model(|| {
        let state: Arc<StackState<u64, ()>> = Arc::new(StackState::new());

        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            // SAFETY: We are the worker; no concurrent access to result slot
            //         before `ready` is set.
            unsafe { worker_state.complete(42) };
            worker_state.wake();
            worker_state.mark_worker_done();
        });

        // Caller spin-polls `is_ready` then takes the result.
        loop {
            if state.is_ready() {
                // SAFETY: ready observed; protocol allows take_outcome.
                let r = unsafe { state.take_outcome() }.unwrap().unwrap();
                assert_eq!(r, 42);
                break;
            }
            thread::yield_now();
        }

        worker.join().unwrap();
        // The drop guard waits for `worker_done`. After worker.join(), step 3
        // has executed, so this drop must return promptly.
        drop(state);
    });
}

/// Caller registers a waker; worker wakes it. The flag must be observed `true`.
#[test]
fn wake_after_set() {
    loom::model(|| {
        let state: Arc<StackState<u8, ()>> = Arc::new(StackState::new());
        let flag = Arc::new(AtomicBool::new(false));

        state.set_waker(&waker_from(&flag));

        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            // SAFETY: exclusive access to result slot until ready is set.
            unsafe { worker_state.complete(7) };
            worker_state.wake();
            worker_state.mark_worker_done();
        });

        worker.join().unwrap();

        assert!(state.is_ready());
        assert!(flag.load(LoomOrdering::SeqCst), "wake() must have flipped the flag");
        // SAFETY: ready observed.
        assert_eq!(unsafe { state.take_outcome() }.unwrap().unwrap(), 7);
        drop(state);
    });
}

/// Caller cancels before any worker is dispatched. `abandon()` releases the
/// drop guard so destruction does not spin forever.
#[test]
fn abandoned_state_drops_without_worker() {
    loom::model(|| {
        let state: StackState<u64, ()> = StackState::new();
        state.abandon();
        // Must return without waiting for a (non-existent) worker.
        drop(state);
    });
}

/// Worker panics: `mark_panicked` writes the Err variant into the outcome
/// slot and flips `ready` Release; `mark_worker_done` releases the guard.
/// The caller observes the Err variant after a successful Acquire on `ready`.
#[test]
fn panic_publish_then_drop() {
    loom::model(|| {
        let state: Arc<StackState<u64, ()>> = Arc::new(StackState::new());

        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            // SAFETY: sole writer of the outcome slot in this test.
            unsafe { worker_state.mark_panicked(Box::new("boom")) };
            worker_state.wake();
            worker_state.mark_worker_done();
        });

        loop {
            if state.is_ready() {
                // SAFETY: ready observed; protocol allows take_outcome.
                let outcome = unsafe { state.take_outcome() }.expect("outcome present");
                assert!(outcome.is_err(), "expected Err variant from mark_panicked");
                break;
            }
            thread::yield_now();
        }

        worker.join().unwrap();
        drop(state);
    });
}

/// AtomicWaker race: caller registers a waker; worker concurrently completes
/// and wakes. The flag must be observed `true` after the caller follows the
/// canonical "register then re-check ready" protocol that `ThunkFuture::poll`
/// uses. (AtomicWaker alone is allowed to drop a bare `wake()` that races
/// with `register()`; the caller's re-check is what closes the race.)
#[test]
fn atomic_waker_register_wake_race() {
    loom::model(|| {
        let state: Arc<StackState<u8, ()>> = Arc::new(StackState::new());
        let flag = Arc::new(AtomicBool::new(false));

        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            // SAFETY: sole writer of the outcome slot in this test.
            unsafe { worker_state.complete(1) };
            worker_state.wake();
            worker_state.mark_worker_done();
        });

        // Mirror ThunkFuture::poll: check, register, re-check.
        let observed_ready = if state.is_ready() {
            true
        } else {
            state.set_waker(&waker_from(&flag));
            state.is_ready()
        };

        worker.join().unwrap();

        // If we observed ready synchronously we may never see the wake fire
        // (and that's fine — Future::poll returned Ready already). Otherwise
        // the wake must propagate to our registered waker.
        if !observed_ready {
            assert!(
                flag.load(LoomOrdering::SeqCst),
                "register-then-recheck protocol must not drop the wake"
            );
        }

        drop(state);
    });
}

/// Two callers race to register a waker on the same `StackState`. Only one
/// can win the `compare_exchange(WAITING, REGISTERING)`; the other must fall
/// through `register`'s `Err` arm and invoke `wake_by_ref` on its own waker
/// directly so no notification is silently dropped. After both registers
/// settle and the worker fires `wake()`, at least one waker flag must be
/// observed `true` — the union of "stored-waker fired" and "fallback
/// wake_by_ref fired" must cover the wake.
#[test]
fn concurrent_registers_race() {
    loom::model(|| {
        let state: Arc<StackState<u8, ()>> = Arc::new(StackState::new());
        let flag_a = Arc::new(AtomicBool::new(false));
        let flag_b = Arc::new(AtomicBool::new(false));

        let state_a = Arc::clone(&state);
        let flag_a_for_thread = Arc::clone(&flag_a);
        let t_a = thread::spawn(move || {
            state_a.set_waker(&waker_from(&flag_a_for_thread));
        });

        state.set_waker(&waker_from(&flag_b));
        t_a.join().unwrap();

        // Now publish: complete + wake + mark_worker_done.
        // SAFETY: sole writer of the outcome slot in this test.
        unsafe { state.complete(9) };
        state.wake();
        state.mark_worker_done();

        // Either flag_a fired (via its own register's fallback wake_by_ref,
        // or via being the stored waker when wake() ran) or flag_b fired
        // (same conditions). Both registers + one wake means *at least one*
        // notification must reach the user, otherwise we lost a wake.
        let a = flag_a.load(LoomOrdering::SeqCst);
        let b = flag_b.load(LoomOrdering::SeqCst);
        assert!(a || b, "concurrent registers + wake must notify at least one waker");

        // SAFETY: ready observed via the worker's complete() above.
        let _ = unsafe { state.take_outcome() };
        drop(state);
    });
}

/// Drop races a concurrent `wake()`. The destructor's spin guard must block
/// until *after* the worker's full `wake()` call (and the subsequent
/// `mark_worker_done`) returns — otherwise the worker would be reading freed
/// memory while invoking `Waker::wake`. This is the most safety-critical
/// race in the crate: a regression here is a UAF on the caller's storage.
#[test]
fn drop_during_wake() {
    loom::model(|| {
        let state: Arc<StackState<u32, ()>> = Arc::new(StackState::new());
        let flag = Arc::new(AtomicBool::new(false));
        state.set_waker(&waker_from(&flag));

        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            // SAFETY: sole writer of the outcome slot.
            unsafe { worker_state.complete(7) };
            worker_state.wake();
            worker_state.mark_worker_done();
        });

        // Drop concurrently with the worker. The destructor must spin on
        // `worker_done` and not return until `mark_worker_done` has been
        // called — which the worker does AFTER `wake()` returns. So even
        // if the worker is mid-`wake()` when we hit `drop`, we are
        // guaranteed to wait it out.
        drop(state);

        worker.join().unwrap();
        // Whether the wake fired or not depends on register/wake ordering;
        // the soundness property under test is that drop didn't return early
        // (loom would have flagged the data race).
        let _ = flag.load(LoomOrdering::SeqCst);
    });
}

/// A second `register` arrives while the worker is mid-`wake()` (holding the
/// `WAKING` bit). The first registered waker has already been taken and
/// fired; the second register sees a non-`WAITING` state, takes its CAS's
/// `Err` arm, and invokes its own `waker.wake_by_ref()` directly. Verifies
/// that the late register's notification is never silently dropped on the
/// floor.
///
/// Loom is asked to explore all interleavings; we assert the invariant
/// "after all threads settle, at least one of the two registered wakers has
/// observed a wake" — the soundness contract of AtomicWaker.
#[test]
fn register_during_wake_falls_back() {
    loom::model(|| {
        let state: Arc<StackState<u8, ()>> = Arc::new(StackState::new());
        let flag_first = Arc::new(AtomicBool::new(false));
        let flag_late = Arc::new(AtomicBool::new(false));

        // First registration happens synchronously before any worker activity.
        state.set_waker(&waker_from(&flag_first));

        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            // SAFETY: sole writer of the outcome slot.
            unsafe { worker_state.complete(3) };
            worker_state.wake();
            worker_state.mark_worker_done();
        });

        // Late registration races against the worker's `wake()`.
        let state_late = Arc::clone(&state);
        let flag_late_for_thread = Arc::clone(&flag_late);
        let t_late = thread::spawn(move || {
            state_late.set_waker(&waker_from(&flag_late_for_thread));
        });

        worker.join().unwrap();
        t_late.join().unwrap();

        // At least one of the two wakers must have observed the notification.
        // - If the late register ran BEFORE wake(): it replaced flag_first as
        //   the stored waker; wake() takes & fires flag_late.
        // - If the late register ran AFTER wake() drained the slot: it sees
        //   WAITING again, stores flag_late, no immediate wake (but the
        //   earlier wake already fired flag_first).
        // - If the late register collides with the WAKING bit: it hits the
        //   Err arm and calls flag_late.wake_by_ref() directly.
        let first = flag_first.load(LoomOrdering::SeqCst);
        let late = flag_late.load(LoomOrdering::SeqCst);
        assert!(
            first || late,
            "register-during-wake protocol must notify at least one of the two registered wakers"
        );

        // SAFETY: ready observed via the worker's complete() above.
        let _ = unsafe { state.take_outcome() };
        drop(state);
    });
}

/// `set_task` (caller side, pre-dispatch) followed by `take_task` (worker
/// side). Although the production protocol requires these to be
/// strictly sequenced via the channel hand-off, modeling them under loom
/// guards against future relaxation of that invariant and proves that the
/// `UnsafeCell` access in `with_mut` is sound for the sequential pattern.
#[test]
fn set_task_then_take_task() {
    loom::model(|| {
        let state: Arc<StackState<(), u64>> = Arc::new(StackState::new());

        // Producer: write the task.
        let prod_state = Arc::clone(&state);
        let producer = thread::spawn(move || {
            // SAFETY: producer-side exclusive access prior to hand-off.
            unsafe { prod_state.set_task(42) };
        });
        producer.join().unwrap();

        // Consumer (worker): hand-off has occurred via the join() above,
        // mirroring the channel-send happens-before in production.
        let cons_state = Arc::clone(&state);
        let consumer = thread::spawn(move || {
            // SAFETY: producer joined; consumer now has exclusive access.
            let task = unsafe { cons_state.take_task() };
            assert_eq!(task, Some(42));
            // SAFETY: sole writer of the outcome slot in this test.
            unsafe { cons_state.complete(()) };
            cons_state.mark_worker_done();
        });
        consumer.join().unwrap();

        assert!(state.is_ready());
        // SAFETY: ready observed.
        let _ = unsafe { state.take_outcome() }.unwrap().unwrap();
        drop(state);
    });
}
