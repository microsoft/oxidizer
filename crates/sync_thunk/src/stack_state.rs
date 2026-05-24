// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::ops::Deref;
use std::task::Waker;

use crate::internal::sync::{AtomicBool, AtomicUsize, Ordering, UnsafeCell, spin_loop_hint, yield_now};
// On the non-loom path we need this extension trait in scope so `.with_mut(...)`
// resolves on the std `UnsafeCell`. Under loom the inherent method exists, so
// the trait is unnecessary there.
#[cfg(not(loom))]
use crate::internal::sync::UnsafeCellExt as _UnsafeCellExt;

// =============================================================================
// CachePadded
// =============================================================================

/// Aligns and pads a value to 64 bytes (a common cache-line size) so that
/// independent writers to adjacent fields in [`StackState`] don't trigger
/// false-sharing cache invalidations on every store.
///
/// Hand-rolled to avoid pulling in `crossbeam-utils` for ~10 lines of code.
/// 64 is a portable approximation: `x86_64` / `aarch64` use 64; Apple silicon
/// and some POWER chips use 128. Over-aligning is harmless; under-aligning
/// only loses some of the perf benefit and never affects correctness.
#[repr(C, align(64))]
struct CachePadded<T>(T);

impl<T> CachePadded<T> {
    fn new(value: T) -> Self {
        Self(value)
    }
}

impl<T> Deref for CachePadded<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}

// =============================================================================
// AtomicWaker
// =============================================================================

const WAITING: usize = 0;
const REGISTERING: usize = 1;
const WAKING: usize = 2;

/// Lock-free single-slot waker storage modeled on the well-known
/// futures-util `AtomicWaker` algorithm. Hand-rolled rather than imported to
/// keep `sync_thunk` free of the `futures` crate dependency.
///
/// Three observable states:
///
/// - [`WAITING`] — no operation in flight; the slot holds either no waker or
///   the last registered waker.
/// - [`REGISTERING`] — a caller is mid-`register()`; it holds exclusive
///   access to the cell until it CAS-releases back to `WAITING`.
/// - [`WAKING`] — a wake is in flight or was observed during a registration
///   and must be re-issued.
///
/// Both flags can be set simultaneously (the state is bitfield-style),
/// allowing `wake` to mark "wake me when you're done registering" by
/// `fetch_or(WAKING)` while a registration is mid-flight.
pub(crate) struct AtomicWaker {
    state: AtomicUsize,
    waker: UnsafeCell<Option<Waker>>,
}

// SAFETY: Access to the cell is gated by the state machine. The REGISTERING
// state grants exclusive write access; the WAKING state grants exclusive
// read+take access. The transitions are CAS-ordered with Acquire/Release.
unsafe impl Send for AtomicWaker {}
// SAFETY: Same as above — the state machine + Acquire/Release CASes make
// the single waker slot safe to share across threads.
unsafe impl Sync for AtomicWaker {}

impl AtomicWaker {
    fn new() -> Self {
        Self {
            state: AtomicUsize::new(WAITING),
            waker: UnsafeCell::new(None),
        }
    }

    /// Registers `waker` to be notified by the next `wake()`. If the same
    /// waker is already stored, skips the `clone()`.
    pub(crate) fn register(&self, waker: &Waker) {
        match self
            .state
            .compare_exchange(WAITING, REGISTERING, Ordering::Acquire, Ordering::Acquire)
        {
            Ok(_) => {
                // We hold REGISTERING exclusively. Write the waker (skipping
                // the clone if the previously stored one is equivalent).
                // SAFETY: REGISTERING state grants exclusive access to the
                // cell until we transition back below.
                self.waker.with_mut(|slot| unsafe {
                    let existing = &mut *slot;
                    if existing.as_ref().is_none_or(|prev| !prev.will_wake(waker)) {
                        *existing = Some(waker.clone());
                    }
                });

                // Release REGISTERING. If a concurrent `wake` happened during
                // the registration window, the state will have transitioned
                // to REGISTERING|WAKING (== 3); take the waker we just stored
                // and fire it ourselves, then drop back to WAITING.
                if self
                    .state
                    .compare_exchange(REGISTERING, WAITING, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    // The only other possibility is REGISTERING|WAKING.
                    // SAFETY: we still hold REGISTERING exclusively
                    // (the wake side only sets the WAKING bit, never
                    // takes the cell).
                    let taken = self.waker.with_mut(|slot| unsafe { (*slot).take() });
                    self.state.store(WAITING, Ordering::Release);
                    if let Some(w) = taken {
                        w.wake();
                    }
                }
            }
            Err(_) => {
                // Either another register is in flight (last-writer-wins is
                // fine for our use — the redundant registration just causes
                // an extra spurious poll, never lost progress), or a wake is
                // currently happening. In both cases, wake the caller-supplied
                // waker directly so we never lose the notification.
                waker.wake_by_ref();
            }
        }
    }

    /// Wakes the registered waker, if any. Idempotent.
    pub(crate) fn wake(&self) {
        let prev = self.state.fetch_or(WAKING, Ordering::AcqRel);
        if prev == WAITING {
            // SAFETY: WAKING bit grants exclusive access to the cell until
            // we clear it. A concurrent `register` will observe WAKING and
            // re-wake on its own path.
            let taken = self.waker.with_mut(|slot| unsafe { (*slot).take() });
            self.state.fetch_and(!WAKING, Ordering::Release);
            if let Some(w) = taken {
                w.wake();
            }
        }
        // else: a register is in flight; that register's exit CAS will fail
        // and it will take the waker + fire it.
    }
}

impl core::fmt::Debug for AtomicWaker {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AtomicWaker").finish_non_exhaustive()
    }
}

// =============================================================================
// StackState
// =============================================================================

/// Internal: hot block that the worker writes once and the poller reads once.
///
/// `ready` becomes `true` after `outcome` is fully written; the Release on
/// `ready` synchronises-with the poller's Acquire load, guaranteeing the
/// outcome write is visible.
struct Body<R> {
    ready: AtomicBool,
    outcome: UnsafeCell<Option<Result<R, Box<dyn Any + Send>>>>,
}

/// Stack-allocated state shared between the async caller and the worker thread.
///
/// # Lifetime Protocol
///
/// Two atomic flags coordinate hand-off between the async poller and the
/// worker thread:
///
/// - [`is_ready`](Self::is_ready) becomes `true` once the worker has written
///   the outcome (success or panic payload). The poller may then take it.
/// - [`is_worker_done`](Self::is_worker_done) becomes `true` once the worker
///   has *fully* stopped touching the state (including waker notification).
///   [`Drop`] spin-loops on this flag — *not* on `ready` — so the worker can
///   safely run `wake()` after publishing the outcome without racing the
///   caller's destructor.
///
/// # Cancellation Safety
///
/// Dropping a `StackState` whose work item was **never dispatched** would
/// otherwise block the dropping thread indefinitely (no worker will ever set
/// `worker_done`). Call [`abandon`](Self::abandon) on the panic path before
/// the state is dropped to release the guard.
///
/// # Layout
///
/// Fields are grouped onto distinct cache lines to avoid false sharing
/// between the worker (which writes the outcome + ready flag + `worker_done`
/// flag) and the caller's Drop spin loop (which reads `worker_done` in a
/// tight loop). Without padding, the worker's `ready` and `outcome` writes
/// would invalidate the cache line the caller is spinning on, multiplying
/// Drop-path latency by tens of nanoseconds per spin.
pub struct StackState<R, T> {
    // Worker writes once when the body completes; caller spins on this in
    // Drop and reads it via `is_ready()` during poll. On its own cache line.
    body: CachePadded<Body<R>>,
    // Worker writes once at the very end of the dispatch lifecycle; caller's
    // Drop spins on this. Placed on its own cache line so the worker's prior
    // writes to `body` don't invalidate the line the spinning caller is
    // watching, and vice versa.
    worker_done: CachePadded<AtomicBool>,
    // Bidirectional: caller registers via `set_waker`, worker calls `wake`.
    // AtomicWaker has its own state machine — no Mutex, no allocation.
    waker: AtomicWaker,
    // Caller writes once before dispatch; worker reads exactly once.
    task: UnsafeCell<Option<T>>,
}

impl<R, T> StackState<R, T> {
    /// Creates a new empty `StackState`.
    pub fn new() -> Self {
        Self {
            body: CachePadded::new(Body {
                ready: AtomicBool::new(false),
                outcome: UnsafeCell::new(None),
            }),
            worker_done: CachePadded::new(AtomicBool::new(false)),
            waker: AtomicWaker::new(),
            task: UnsafeCell::new(None),
        }
    }

    /// Stores the task arguments into the state.
    ///
    /// # Safety
    ///
    /// Must not be called concurrently with [`take_task`](Self::take_task).
    pub unsafe fn set_task(&self, task: T) {
        // SAFETY: Caller guarantees no concurrent access to the task slot.
        self.task.with_mut(|slot| unsafe { *slot = Some(task) });
    }

    /// Takes the task arguments out of the state, returning `None` if already taken.
    ///
    /// # Safety
    ///
    /// Must not be called concurrently with [`set_task`](Self::set_task).
    pub unsafe fn take_task(&self) -> Option<T> {
        // SAFETY: Caller guarantees no concurrent access to the task slot.
        self.task.with_mut(|slot| unsafe { (*slot).take() })
    }

    /// Writes the computed result and signals readiness.
    ///
    /// # Safety
    ///
    /// Must be called exactly once by the worker thread after computing the result.
    pub unsafe fn complete(&self, result: R) {
        // SAFETY: Caller guarantees exclusive access to the outcome slot at this point.
        self.body.outcome.with_mut(|slot| unsafe { *slot = Some(Ok(result)) });
        self.body.ready.store(true, Ordering::Release);
    }

    /// Marks the state as panicked, stores the captured panic payload, and
    /// signals readiness.
    ///
    /// `ThunkFuture::poll` will subsequently take the payload and hand it to
    /// `std::panic::resume_unwind`, re-raising the original panic on the
    /// awaiter's task with the same value (preserving downcastable types
    /// like `String`, `&'static str`, or custom payloads). The worker must
    /// still call [`mark_worker_done`](Self::mark_worker_done) afterwards.
    ///
    /// # Safety
    ///
    /// Must be called exactly once by the worker thread, and never
    /// concurrently with [`take_outcome`](Self::take_outcome).
    pub unsafe fn mark_panicked(&self, payload: Box<dyn Any + Send>) {
        // SAFETY: Caller guarantees exclusive access to the outcome slot —
        // the poller only reads it after observing `ready`.
        self.body.outcome.with_mut(|slot| unsafe { *slot = Some(Err(payload)) });
        self.body.ready.store(true, Ordering::Release);
    }

    /// Takes the worker's outcome — `Ok(R)` on success or `Err(payload)` on
    /// panic — out of the state.
    ///
    /// # Safety
    ///
    /// Must only be called after [`is_ready`](Self::is_ready) returns `true`
    /// and at most once.
    pub unsafe fn take_outcome(&self) -> Option<Result<R, Box<dyn Any + Send>>> {
        // SAFETY: ready==true synchronises-with the worker's outcome write;
        // caller guarantees exclusive access.
        self.body.outcome.with_mut(|slot| unsafe { (*slot).take() })
    }

    /// Returns `true` if the worker has signaled completion.
    pub fn is_ready(&self) -> bool {
        self.body.ready.load(Ordering::Acquire)
    }

    /// Returns `true` once the worker has fully released the state.
    ///
    /// Callers must observe this as `true` (or guarantee no worker will ever
    /// run, via [`abandon`](Self::abandon)) before the state is dropped.
    pub fn is_worker_done(&self) -> bool {
        self.worker_done.load(Ordering::Acquire)
    }

    /// Signals that the worker has fully finished touching the state.
    ///
    /// Must be the **last** operation the worker performs on the state. After
    /// this returns, the caller's [`Drop`] is free to deallocate the storage.
    pub fn mark_worker_done(&self) {
        self.worker_done.store(true, Ordering::Release);
    }

    /// Releases the `Drop` spin-guard without a worker ever having run.
    ///
    /// Use this on the panic path between `StackState::new()` and successful
    /// dispatch of the work item. After `abandon`, dropping the state is
    /// non-blocking. Must not be called once the work item is in flight.
    pub fn abandon(&self) {
        self.body.ready.store(true, Ordering::Release);
        self.worker_done.store(true, Ordering::Release);
    }

    /// Registers a waker to be notified when the result is ready.
    ///
    /// Cheaper than the historical `Mutex<Option<Waker>>` approach: no lock
    /// acquisition, no allocation, and skips the `Waker::clone()` when the
    /// caller re-registers the same waker on a re-poll (the common case for
    /// long-running blocking work polled by a busy executor).
    pub fn set_waker(&self, waker: &Waker) {
        self.waker.register(waker);
    }

    /// Wakes the registered waker, if present. Safe to call multiple times.
    pub fn wake(&self) {
        self.waker.wake();
    }

    /// Returns a raw const pointer to this `StackState`.
    pub fn as_ptr(&self) -> *const Self {
        self
    }

    /// Returns a raw mutable pointer to this `StackState`.
    pub fn as_mut_ptr(&self) -> *mut Self {
        std::ptr::from_ref(self).cast_mut()
    }
}

impl<R, T> Default for StackState<R, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R, T> core::fmt::Debug for StackState<R, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StackState")
            .field("ready", &self.body.ready.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<R, T> Drop for StackState<R, T> {
    fn drop(&mut self) {
        // Cancellation guard: prevent use-after-free if the future is dropped
        // before the worker finishes touching our stack-allocated state.
        // We wait on `worker_done` (not `ready`) so the worker is free to
        // perform waker notification *after* publishing the outcome without
        // racing this destructor.
        //
        // Escalation ladder: a short PAUSE/yield burst handles the common
        // case where the worker finishes within microseconds. After that
        // we yield to the OS, and finally fall back to short sleeps so a
        // slow worker (e.g. blocked in a long syscall) doesn't pin a CPU
        // core at 100 % for the duration. Under `cfg(loom)` we keep
        // yielding indefinitely — sleep would interact poorly with loom's
        // deterministic scheduler.
        let mut spins: u32 = 0;
        while !self.worker_done.load(Ordering::Acquire) {
            if spins < 64 {
                spin_loop_hint();
            } else if spins < 1024 {
                yield_now();
            } else {
                #[cfg(not(loom))]
                std::thread::sleep(std::time::Duration::from_millis(1));
                #[cfg(loom)]
                yield_now();
            }
            spins = spins.saturating_add(1);
        }
    }
}

// SAFETY: StackState is designed for cross-thread sharing between an async
// poller and a worker thread. Access to UnsafeCell fields is synchronized
// by the `ready` atomic flag, the worker_done atomic flag, and the protocol
// enforced by the unsafe methods.
unsafe impl<R: Send, T: Send> Sync for StackState<R, T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;
    use std::task::{RawWaker, RawWakerVTable};

    use super::*;

    fn noop_clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &NOOP_VTABLE)
    }
    fn noop(_: *const ()) {}
    static NOOP_VTABLE: RawWakerVTable = RawWakerVTable::new(noop_clone, noop, noop, noop);

    /// Creates a no-op waker for testing.
    fn noop_waker() -> Waker {
        // SAFETY: The vtable functions are sound no-ops.
        unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &NOOP_VTABLE)) }
    }

    fn flag_clone(data: *const ()) -> RawWaker {
        // SAFETY: data points to a valid Arc<AtomicBool>.
        let arc = unsafe { Arc::from_raw(data.cast::<std::sync::atomic::AtomicBool>()) };
        let clone = Arc::clone(&arc);
        core::mem::forget(arc);
        RawWaker::new(Arc::into_raw(clone).cast(), &FLAG_VTABLE)
    }
    fn flag_wake(data: *const ()) {
        // SAFETY: data points to a valid Arc<AtomicBool>.
        let arc = unsafe { Arc::from_raw(data.cast::<std::sync::atomic::AtomicBool>()) };
        arc.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    fn flag_wake_by_ref(data: *const ()) {
        // SAFETY: data points to a valid Arc<AtomicBool>.
        let arc = unsafe { Arc::from_raw(data.cast::<std::sync::atomic::AtomicBool>()) };
        arc.store(true, std::sync::atomic::Ordering::SeqCst);
        core::mem::forget(arc);
    }
    fn flag_drop(data: *const ()) {
        // SAFETY: data points to a valid Arc<AtomicBool>.
        unsafe { drop(Arc::from_raw(data.cast::<std::sync::atomic::AtomicBool>())) };
    }
    static FLAG_VTABLE: RawWakerVTable = RawWakerVTable::new(flag_clone, flag_wake, flag_wake_by_ref, flag_drop);

    #[test]
    fn new_is_not_ready() {
        let state = StackState::<u32, u32>::new();
        assert!(!state.is_ready());
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(0) };
        state.mark_worker_done();
    }

    #[test]
    fn default_is_not_ready() {
        let state = StackState::<u32, u32>::default();
        assert!(!state.is_ready());
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(0) };
        state.mark_worker_done();
    }

    #[test]
    fn set_and_take_task() {
        let state = StackState::<(), String>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.set_task(String::from("hello")) };
        // SAFETY: No concurrent access — single-threaded test.
        let task = unsafe { state.take_task() };
        assert_eq!(task.as_deref(), Some("hello"));
        // SAFETY: No concurrent access — single-threaded test.
        let task2 = unsafe { state.take_task() };
        assert!(task2.is_none());
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(()) };
        state.mark_worker_done();
    }

    #[test]
    fn complete_and_take_outcome() {
        let state = StackState::<String, ()>::new();
        assert!(!state.is_ready());
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(String::from("result")) };
        state.mark_worker_done();
        assert!(state.is_ready());
        // SAFETY: is_ready() returned true; no concurrent access.
        let val = unsafe { state.take_outcome() };
        assert!(matches!(val, Some(Ok(ref s)) if s == "result"));
        // SAFETY: No concurrent access — single-threaded test.
        let val2 = unsafe { state.take_outcome() };
        assert!(val2.is_none());
    }

    #[test]
    fn mark_panicked_records_err_outcome() {
        let state = StackState::<u32, ()>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.mark_panicked(Box::new("kaboom") as Box<dyn Any + Send>) };
        state.mark_worker_done();
        assert!(state.is_ready());
        // SAFETY: ready observed; no concurrent access.
        let outcome = unsafe { state.take_outcome() }.expect("outcome present");
        let err = outcome.expect_err("err variant");
        let msg = err.downcast::<&'static str>().expect("string payload");
        assert_eq!(*msg, "kaboom");
    }

    #[test]
    fn set_waker_and_wake() {
        let woken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let woken2 = Arc::clone(&woken);

        let raw = RawWaker::new(Arc::into_raw(woken2).cast(), &FLAG_VTABLE);
        // SAFETY: The vtable functions correctly manage Arc refcounts.
        let waker = unsafe { Waker::from_raw(raw) };

        let state = StackState::<(), ()>::new();
        state.set_waker(&waker);
        state.wake();
        assert!(woken.load(std::sync::atomic::Ordering::SeqCst));
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(()) };
        state.mark_worker_done();
    }

    #[test]
    fn wake_without_waker_is_noop() {
        let state = StackState::<(), ()>::new();
        state.wake();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(()) };
        state.mark_worker_done();
    }

    #[test]
    fn re_registering_same_waker_skips_clone() {
        // Smoke test: register-twice doesn't panic and a subsequent wake still
        // fires. (The will_wake optimization is internal; we test observable
        // behavior here.)
        let state = StackState::<(), ()>::new();
        let woken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let raw = RawWaker::new(Arc::into_raw(Arc::clone(&woken)).cast(), &FLAG_VTABLE);
        // SAFETY: vtable manages Arc lifetimes.
        let waker = unsafe { Waker::from_raw(raw) };
        state.set_waker(&waker);
        state.set_waker(&waker);
        state.wake();
        assert!(woken.load(std::sync::atomic::Ordering::SeqCst));
        // SAFETY: single-threaded.
        unsafe { state.complete(()) };
        state.mark_worker_done();
    }

    #[test]
    fn as_ptr_and_as_mut_ptr() {
        let state = StackState::<u32, u32>::new();
        let p = state.as_ptr();
        let mp = state.as_mut_ptr();
        assert_eq!(p, mp.cast_const());
        assert_eq!(p, &raw const state);
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(0) };
        state.mark_worker_done();
    }

    #[test]
    fn debug_impl_not_ready() {
        let state = StackState::<u32, u32>::new();
        let debug = format!("{state:?}");
        assert!(debug.contains("StackState"));
        assert!(debug.contains("false"));
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(0) };
        state.mark_worker_done();
    }

    #[test]
    fn debug_impl_ready() {
        let state = StackState::<u32, u32>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(42) };
        state.mark_worker_done();
        let debug = format!("{state:?}");
        assert!(debug.contains("true"));
    }

    #[test]
    fn drop_blocks_until_ready() {
        let state = StackState::<u32, u32>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(99) };
        state.mark_worker_done();
        drop(state);
    }

    #[test]
    fn sync_trait_bounds() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<StackState<u32, u32>>();
    }

    #[test]
    fn set_waker_with_noop() {
        let state = StackState::<(), ()>::new();
        state.set_waker(&noop_waker());
        state.wake();
        state.wake();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(()) };
        state.mark_worker_done();
    }

    #[test]
    fn complete_with_complex_type() {
        let state = StackState::<Vec<String>, ()>::new();
        let data = vec![String::from("a"), String::from("b")];
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(data) };
        state.mark_worker_done();
        assert!(state.is_ready());
        // SAFETY: is_ready() returned true; no concurrent access.
        let result = unsafe { state.take_outcome() }.unwrap().unwrap();
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn task_with_complex_type() {
        let state = StackState::<(), Vec<u32>>::new();
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.set_task(vec![1, 2, 3]) };
        // SAFETY: No concurrent access — single-threaded test.
        let task = unsafe { state.take_task() }.unwrap();
        assert_eq!(task, vec![1, 2, 3]);
        // SAFETY: No concurrent access — single-threaded test.
        unsafe { state.complete(()) };
        state.mark_worker_done();
    }

    #[test]
    fn abandon_releases_drop_guard() {
        let state = StackState::<u32, u32>::new();
        assert!(!state.is_ready());
        assert!(!state.is_worker_done());
        state.abandon();
        assert!(state.is_ready());
        assert!(state.is_worker_done());
        // Drop must not block.
        drop(state);
    }

    #[test]
    fn worker_done_independent_of_ready() {
        let state = StackState::<u32, u32>::new();
        // SAFETY: single-threaded.
        unsafe { state.complete(7) };
        assert!(state.is_ready());
        assert!(!state.is_worker_done());
        state.mark_worker_done();
        assert!(state.is_worker_done());
    }

    #[test]
    fn cache_padded_alignment() {
        // Verify the body and worker_done fields actually live on distinct
        // cache lines. The CachePadded<_> wrapper forces 64-byte alignment;
        // a packed-but-unpadded layout would let them straddle a cache line.
        let state = StackState::<u32, u32>::new();
        let body_addr = std::ptr::from_ref(&*state.body).addr();
        let worker_done_addr = std::ptr::from_ref(&*state.worker_done).addr();
        assert_eq!(body_addr % 64, 0, "body block must be 64B-aligned");
        assert_eq!(worker_done_addr % 64, 0, "worker_done block must be 64B-aligned");
        // SAFETY: single-threaded.
        unsafe { state.complete(0) };
        state.mark_worker_done();
    }

    #[test]
    fn atomic_waker_debug_impl() {
        let waker = AtomicWaker::new();
        let debug = format!("{waker:?}");
        assert!(debug.contains("AtomicWaker"));
    }

    /// Race `register` against concurrent `wake()` calls to drive the
    /// `register` CAS into its `Err` arm (state observed as `WAKING` or
    /// `REGISTERING`). With ~10k iterations on two threads the race is
    /// almost certain to land at least one `register` call inside that
    /// path, covering lines 128-135.
    #[test]
    fn atomic_waker_register_loses_cas_under_contention() {
        let aw = Arc::new(AtomicWaker::new());
        let aw2 = Arc::clone(&aw);

        let waker = noop_waker();
        let waker2 = waker.clone();

        let waker_thread = std::thread::spawn(move || {
            for _ in 0..10_000 {
                aw2.wake();
            }
        });
        for _ in 0..10_000 {
            aw.register(&waker);
        }
        waker_thread.join().unwrap();
        // Final wake/register to leave the waker in a clean state.
        aw.register(&waker2);
        aw.wake();
    }

    /// Exercise the spin → yield → sleep escalation ladder in `Drop` by
    /// keeping a worker thread "alive but not yet done" long enough that the
    /// dropper has to fall into the 1ms-sleep arm. Verifies that the
    /// destructor blocks until the worker eventually calls
    /// `mark_worker_done()` rather than returning early.
    #[test]
    fn drop_spin_loop_escalates_to_sleep() {
        // The StackState must have a stable address: the worker thread reads
        // it through a raw pointer while the destructor is running. Calling
        // `drop(stack_local)` would move the value into the `drop` fn's
        // parameter slot, leaving the worker writing to a stale address.
        // `Box` pins the value on the heap so dropping the Box runs
        // `drop_in_place` at the same address the worker observes.
        let state = Box::new(StackState::<u32, ()>::new());
        // SAFETY: only this thread writes to `state.body` via complete().
        unsafe { state.complete(7) };

        let ptr: *const StackState<u32, ()> = Box::as_ref(&state);
        ptr.expose_provenance();
        let addr = ptr.addr();
        let handle = std::thread::spawn(move || {
            // Hold the not-done state long enough to push the dropper past
            // the spin-loop (64) and yield-loop (1024) tiers into the
            // 1 ms-sleep arm.
            std::thread::sleep(std::time::Duration::from_millis(20));
            // SAFETY: `state` outlives this access — its destructor (still
            // executing on the main thread) is what we are racing, and that
            // destructor will not return until `mark_worker_done` below
            // completes.
            let s = unsafe { &*std::ptr::with_exposed_provenance::<StackState<u32, ()>>(addr) };
            s.mark_worker_done();
        });

        // Dropping the Box runs StackState::drop at `addr`, which spins
        // (then yields, then sleeps) until the worker flips `worker_done`.
        drop(state);
        handle.join().unwrap();
    }
}
