// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};

use crate::{ThunkerBuilder, ThunkerSender, WorkItem};

/// Shared state between the `Thunker` handle and its worker threads.
pub(crate) struct ThunkerInner {
    pub(crate) sender: Sender<WorkItem>,
    receiver: Receiver<WorkItem>,
    thread_count: AtomicUsize,
    /// Number of live `Thunker` handles. When this transitions to zero,
    /// `shutdown` is set so idle workers exit instead of pinning the
    /// "always keep one worker" rule and leaking a thread per `Thunker`.
    handle_count: AtomicUsize,
    /// Set when the last `Thunker` handle has been dropped.
    shutdown: AtomicBool,
    max_thread_count: usize,
    cool_down_interval: Duration,
}

/// An auto-scaling thread pool for dispatching blocking work from async code.
///
/// `Thunker` manages a pool of worker threads that execute blocking operations on behalf of async tasks.
///
/// # Thread Scaling
///
/// The pool starts with **one** worker thread. When a new work item is enqueued and the
/// number of pending items meets or exceeds the current thread count, an additional worker
/// is spawned (up to [`max_thread_count`](Self::max_thread_count)). The scale-up decision
/// uses a compare-and-swap to prevent multiple threads from being spawned simultaneously.
///
/// Workers that receive no work for [`cool_down_interval`](Self::cool_down_interval) exit
/// voluntarily, but at least one worker is always kept alive.
///
/// # Examples
///
/// Using default settings:
///
/// ```
/// use sync_thunk::Thunker;
///
/// let thunker = Thunker::new();
/// ```
///
/// Using the builder for custom configuration:
///
/// ```
/// use std::time::Duration;
///
/// use sync_thunk::Thunker;
///
/// let thunker = Thunker::builder()
///     .max_thread_count(8)
///     .cool_down_interval(Duration::from_secs(30))
///     .build();
/// ```
pub struct Thunker {
    pub(crate) inner: Arc<ThunkerInner>,
}

impl Clone for Thunker {
    fn clone(&self) -> Self {
        let _ = self.inner.handle_count.fetch_add(1, Ordering::Relaxed);
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Drop for Thunker {
    fn drop(&mut self) {
        // When the last handle goes away, ask all idle workers to exit.
        // Acquire-on-the-fetch so any writes by other handles before drop
        // are visible to workers that observe `shutdown == true`.
        if self.inner.handle_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.shutdown.store(true, Ordering::Release);
        }
    }
}

impl Thunker {
    /// Creates a new `Thunker` with default settings and spawns an initial worker thread.
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Returns a [`ThunkerBuilder`] for configuring a new `Thunker`.
    #[must_use]
    pub fn builder() -> ThunkerBuilder {
        ThunkerBuilder::new()
    }

    /// Constructs a `Thunker` from a completed builder.
    pub(crate) fn from_builder(builder: ThunkerBuilder) -> Self {
        let (sender, receiver) = crossbeam_channel::bounded(builder.channel_capacity);
        let thunker = Self {
            inner: Arc::new(ThunkerInner {
                sender,
                receiver,
                thread_count: AtomicUsize::new(0),
                handle_count: AtomicUsize::new(1),
                shutdown: AtomicBool::new(false),
                max_thread_count: builder.max_thread_count,
                cool_down_interval: builder.cool_down_interval,
            }),
        };
        Self::spawn_worker(&thunker.inner);
        thunker
    }

    /// Returns the maximum number of worker threads.
    #[must_use]
    pub fn max_thread_count(&self) -> usize {
        self.inner.max_thread_count
    }

    /// Returns the cool-down interval for idle worker threads.
    #[must_use]
    pub fn cool_down_interval(&self) -> Duration {
        self.inner.cool_down_interval
    }

    /// Returns the current number of active worker threads.
    #[must_use]
    pub fn thread_count(&self) -> usize {
        self.inner.thread_count.load(Ordering::Relaxed)
    }

    /// Returns a cheap, dispatch-only handle that shares this `Thunker`'s
    /// worker pool. The returned [`ThunkerSender`] increments only the
    /// `Arc<ThunkerInner>` strong count — not the user-facing `handle_count`
    /// that governs shutdown — so the `#[thunk]` macro can clone one per
    /// dispatch without inflating handle bookkeeping on the hot path.
    #[must_use]
    pub fn sender(&self) -> ThunkerSender {
        ThunkerSender {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Sends a work item to be executed on a worker thread.
    ///
    /// Automatically scales up the thread pool if the queue is backing up
    /// and the current thread count is below the configured maximum.
    ///
    /// # Panics
    ///
    /// Panics if the channel is closed.
    #[doc(hidden)]
    pub fn send(&self, item: WorkItem) {
        send_impl(&self.inner, item);
    }

    /// Spawns a worker thread and increments the thread count.
    fn spawn_worker(inner: &Arc<ThunkerInner>) {
        let _ = inner.thread_count.fetch_add(1, Ordering::AcqRel);
        spawn_worker_already_counted(inner);
    }
}

/// Implementation of the dispatch path shared by `Thunker::send` and
/// `ThunkerSender::send`. Reads `sender.len()` (cheap atomic difference inside
/// the channel) as the queue-backlog heuristic instead of maintaining a
/// separate `pending_count` atomic on a contended cache line.
pub(crate) fn send_impl(inner: &Arc<ThunkerInner>, item: WorkItem) {
    let pending = inner.sender.len();
    let threads = inner.thread_count.load(Ordering::Relaxed);

    // Scale up if the queue is backing up and we haven't hit the limit.
    if pending >= threads
        && threads < inner.max_thread_count
        && inner
            .thread_count
            .compare_exchange(threads, threads + 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    {
        spawn_worker_already_counted(inner);
    }

    inner.sender.send(item).unwrap_or_else(|_| {
        // Unreachable: the receiver is co-owned with the sender via
        // `Arc<ThunkerInner>`. As long as any handle exists (we are
        // inside `&self` of one such handle) the receiver is alive,
        // so the channel cannot be in a disconnected state here.
        unreachable!("sync_thunk channel disconnected while a handle is alive")
    });
}

/// Spawns a worker thread, assuming the caller already incremented the count.
fn spawn_worker_already_counted(inner: &Arc<ThunkerInner>) {
    let inner_clone = Arc::clone(inner);
    let spawn_result = std::thread::Builder::new().name("sync-thunk-worker".into()).spawn(move || {
        worker_loop(&inner_clone);
    });
    // Roll back the speculative thread-count bump on failure so the pool
    // can attempt another scale-up later instead of being permanently
    // "stuck at N threads" without any workers.
    if spawn_result.is_err() {
        let _ = inner.thread_count.fetch_sub(1, Ordering::AcqRel);
        #[expect(clippy::panic, reason = "matches previous .expect() behaviour")]
        {
            panic!("OS refused to spawn a sync-thunk worker thread; the system may be out of resources");
        }
    }
}

/// Drains every item currently in the channel and executes it. Called on a
/// worker's shutdown path so that no `#[thunk]` work item is silently
/// discarded — see the call-site comment in [`worker_loop`] for the
/// full rationale. Panics inside `execute()` are absorbed identically to
/// the steady-state loop so a single misbehaving wake closure cannot
/// strand the remaining items.
fn drain_and_execute(inner: &ThunkerInner) {
    while let Ok(item) = inner.receiver.try_recv() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| item.execute()));
    }
}

fn worker_loop(inner: &ThunkerInner) {
    // Defense-in-depth: decrement `thread_count` no matter how the loop
    // exits (normal return, scale-down, shutdown, or a panic escaping
    // the per-iteration `catch_unwind` below). Without this guard, a
    // worker thread that died by panic would leave the pool
    // permanently believing it had more threads than actually exist,
    // gating future scale-up incorrectly.
    //
    // Relaxed is sufficient here: the scale-up CAS is AcqRel and
    // re-establishes ordering for any subsequent reader, and the
    // user-facing `thread_count()` getter is itself Relaxed.
    struct ThreadCountGuard<'a>(&'a AtomicUsize);
    impl Drop for ThreadCountGuard<'_> {
        fn drop(&mut self) {
            let _ = self.0.fetch_sub(1, Ordering::Relaxed);
        }
    }
    let _thread_guard = ThreadCountGuard(&inner.thread_count);

    loop {
        // Drain quickly without waiting if shutdown was requested while
        // we were processing work. Relaxed is fine: a stale `false` just
        // costs one extra `recv_timeout` iteration, which is bounded by
        // `cool_down_interval`.
        //
        // Before exiting, drain every remaining item from the channel and
        // execute it. Each in-flight `#[thunk]` work item owns the
        // responsibility for calling `mark_worker_done` on its caller's
        // `StackState` (via the shim's `__DoneOnDrop` guard); if we
        // returned without executing pending items, those callers would
        // spin forever in `StackState::Drop` waiting for a
        // `mark_worker_done` that never arrives — and any still-awaiting
        // `ThunkFuture` would hang permanently. `WorkItem` itself has no
        // destructor that could release those guards, so the drain MUST
        // happen here, on the worker thread that still holds the channel
        // alive via `Arc<ThunkerInner>`.
        if inner.shutdown.load(Ordering::Relaxed) {
            drain_and_execute(inner);
            return;
        }
        match inner.receiver.recv_timeout(inner.cool_down_interval) {
            Ok(item) => {
                // The shim itself catches panics from the user's
                // `#[thunk]`-annotated body, but the trailing
                // `state.wake()` call inside the shim invokes
                // *caller-supplied* `Waker::wake()` code, which is
                // outside that catch_unwind. If a misbehaving executor
                // panics from `wake()`, the panic would unwind out of
                // `item.execute()`, kill this worker thread, and leave
                // `thread_count` permanently inflated — eventually
                // starving the pool. Absorb it here so the worker
                // survives.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| item.execute()));
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // If the last handle has been dropped, exit unconditionally
                // — otherwise the "keep one worker alive" rule would leak
                // a permanent thread per `Thunker` ever created. Drain
                // first for the same reason as the top-of-loop check
                // above: a `Thunker` dropped between dispatches may have
                // left orphaned items in the channel.
                if inner.shutdown.load(Ordering::Relaxed) {
                    drain_and_execute(inner);
                    return;
                }
                // Scale down: CAS loop ensures at least one worker remains.
                let mut count = inner.thread_count.load(Ordering::Relaxed);
                loop {
                    if count <= 1 {
                        break;
                    }
                    match inner
                        .thread_count
                        .compare_exchange_weak(count, count - 1, Ordering::AcqRel, Ordering::Relaxed)
                    {
                        Ok(_) => {
                            // The ThreadCountGuard will decrement once
                            // more on drop, but we've already accounted
                            // for *this* worker via the successful CAS
                            // — so cancel the guard by adding one back
                            // before returning.
                            let _ = inner.thread_count.fetch_add(1, Ordering::Relaxed);
                            return;
                        }
                        Err(actual) => count = actual,
                    }
                }
                // Last worker — keep running.
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                // Unreachable in practice: `sender` is owned by
                // `ThunkerInner` which workers hold via `Arc`, so the
                // channel cannot disconnect while any worker exists.
                // Kept as a defensive fallback that still exits cleanly.
                debug_assert!(false, "channel disconnected while worker holds Arc");
                return;
            }
        }
    }
}

impl Default for Thunker {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for Thunker {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Thunker")
            .field("max_thread_count", &self.inner.max_thread_count)
            .field("cool_down_interval", &self.inner.cool_down_interval)
            .field("thread_count", &self.inner.thread_count.load(Ordering::Relaxed))
            .field("pending_count", &self.inner.sender.len())
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn set_flag(ptr: *mut ()) {
        // SAFETY: ptr is a valid Arc<AtomicBool> created via Arc::into_raw.
        let arc = unsafe { Arc::from_raw(ptr.cast::<AtomicBool>()) };
        arc.store(true, Ordering::SeqCst);
    }

    fn wait_on_barrier(ptr: *mut ()) {
        // SAFETY: ptr is a valid Arc<Barrier> created via Arc::into_raw.
        let b = unsafe { Arc::from_raw(ptr.cast::<std::sync::Barrier>()) };
        b.wait();
    }

    #[test]
    fn new_has_defaults() {
        let t = Thunker::new();
        assert_eq!(t.max_thread_count(), 4);
        assert_eq!(t.cool_down_interval(), Duration::from_secs(10));
        assert!(t.thread_count() >= 1);
    }

    #[test]
    fn builder_returns_builder() {
        let builder = Thunker::builder();
        let t = builder.max_thread_count(2).build();
        assert_eq!(t.max_thread_count(), 2);
    }

    #[test]
    fn from_builder_spawns_initial_worker() {
        let t = Thunker::builder().build();
        std::thread::sleep(Duration::from_millis(10));
        assert!(t.thread_count() >= 1);
    }

    #[test]
    fn clone_shares_state() {
        let t1 = Thunker::new();
        let t2 = t1.clone();
        assert_eq!(t1.max_thread_count(), t2.max_thread_count());
        assert_eq!(t1.cool_down_interval(), t2.cool_down_interval());
    }

    #[test]
    fn default_same_as_new() {
        let d = Thunker::default();
        let n = Thunker::new();
        assert_eq!(d.max_thread_count(), n.max_thread_count());
        assert_eq!(d.cool_down_interval(), n.cool_down_interval());
    }

    #[test]
    fn debug_impl() {
        let t = Thunker::new();
        let debug = format!("{t:?}");
        assert!(debug.contains("Thunker"));
        assert!(debug.contains("max_thread_count"));
        assert!(debug.contains("cool_down_interval"));
        assert!(debug.contains("thread_count"));
        assert!(debug.contains("pending_count"));
    }

    #[test]
    fn send_executes_work_item() {
        let t = Thunker::new();
        let executed = Arc::new(AtomicBool::new(false));
        let executed2 = Arc::clone(&executed);

        let flag_ptr = Arc::into_raw(executed2).cast_mut().cast::<()>();
        // SAFETY: `flag_ptr` is an `Arc<AtomicBool>` raw pointer; `set_flag`
        // reconstructs the `Arc` and stores into the `AtomicBool`.
        let item = unsafe { WorkItem::new(flag_ptr, set_flag) };
        t.send(item);

        for _ in 0..100 {
            if executed.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(executed.load(Ordering::SeqCst));
    }

    #[test]
    fn send_scales_up_threads() {
        use std::sync::Barrier;

        let t = Thunker::builder().max_thread_count(4).channel_capacity(8).build();

        let barrier = Arc::new(Barrier::new(4));
        for _ in 0..3 {
            let b = Arc::clone(&barrier);
            let b_ptr = Arc::into_raw(b).cast_mut().cast::<()>();
            // SAFETY: `b_ptr` is an `Arc<Barrier>` raw pointer; `wait_on_barrier`
            // reconstructs the `Arc` and calls `wait()`.
            t.send(unsafe { WorkItem::new(b_ptr, wait_on_barrier) });
        }

        std::thread::sleep(Duration::from_millis(100));
        let count = t.thread_count();
        assert!(count >= 2, "expected at least 2 threads, got {count}");

        barrier.wait();
    }

    #[test]
    fn custom_cool_down_interval() {
        let interval = Duration::from_millis(50);
        let t = Thunker::builder().cool_down_interval(interval).build();
        assert_eq!(t.cool_down_interval(), interval);
    }

    #[test]
    fn sender_returns_dispatch_handle() {
        let t = Thunker::new();
        let sender = t.sender();
        let executed = Arc::new(AtomicBool::new(false));
        let executed2 = Arc::clone(&executed);
        let flag_ptr = Arc::into_raw(executed2).cast_mut().cast::<()>();
        // SAFETY: `flag_ptr` is an `Arc<AtomicBool>` raw pointer.
        let item = unsafe { WorkItem::new(flag_ptr, set_flag) };
        sender.send(item);

        for _ in 0..100 {
            if executed.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(executed.load(Ordering::SeqCst));
    }

    type Gate = (std::sync::Mutex<u32>, std::sync::Condvar, AtomicBool);

    fn signal_and_park(ptr: *mut ()) {
        // SAFETY: ptr is a valid `Arc<Gate>` created via Arc::into_raw.
        let arc = unsafe { Arc::from_raw(ptr.cast::<Gate>()) };
        let (lock, cv, released) = &*arc;
        // If main thread has already signalled "release", return immediately
        // so leftover queued items don't re-park a worker after release.
        if released.load(Ordering::Acquire) {
            return;
        }
        let mut started = lock.lock().unwrap();
        *started += 1;
        cv.notify_all();
        while !released.load(Ordering::Acquire) {
            started = cv.wait(started).unwrap();
        }
    }

    #[test]
    fn workers_scale_down_after_idle() {
        // We need at least two live workers concurrently so the timeout
        // arm in `worker_loop` finds `thread_count > 1` and exercises the
        // scale-down CAS loop.
        let t = Thunker::builder()
            .max_thread_count(4)
            .channel_capacity(16)
            .cool_down_interval(Duration::from_millis(20))
            .build();

        let gate: Arc<Gate> = Arc::new((std::sync::Mutex::new(0u32), std::sync::Condvar::new(), AtomicBool::new(false)));

        // Dispatch enough items that the channel backs up and forces
        // `send_impl`'s scale-up CAS to spawn at least one extra worker.
        for _ in 0..8 {
            let g = Arc::clone(&gate);
            let g_ptr = Arc::into_raw(g).cast_mut().cast::<()>();
            // SAFETY: `g_ptr` is an `Arc<Gate>` raw pointer.
            t.send(unsafe { WorkItem::new(g_ptr, signal_and_park) });
        }

        // Wait until 2 workers have actually entered their work items.
        {
            let (lock, cv, _) = &*gate;
            let mut started = lock.lock().unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while *started < 2 {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                assert!(!remaining.is_zero(), "scale-up never produced 2 concurrent workers");
                let (s, _) = cv.wait_timeout(started, remaining).unwrap();
                started = s;
            }
        }

        let peak = t.thread_count();
        assert!(peak >= 2, "expected scale-up to >= 2 threads, observed peak={peak}");

        // Release all parked workers AND tell any not-yet-started item to
        // bail immediately so we don't deadlock on leftover queued work.
        {
            let (lock, cv, released) = &*gate;
            released.store(true, Ordering::Release);
            let _guard = lock.lock().unwrap();
            cv.notify_all();
        }

        // After several cool-down intervals with no further work, the extra
        // worker(s) must observe a `Timeout` and walk the CAS loop down to 1.
        let mut final_count = peak;
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(20));
            final_count = t.thread_count();
            if final_count == 1 {
                break;
            }
        }
        assert_eq!(final_count, 1, "expected scale-down to exactly 1 worker, got {final_count}");
    }

    #[test]
    fn dropping_last_handle_shuts_down_workers() {
        // Short cool-down so the worker wakes from `recv_timeout` quickly and
        // observes the shutdown flag, exercising the `shutdown == true` branch
        // of the timeout arm in `worker_loop`.
        let t = Thunker::builder()
            .max_thread_count(1)
            .cool_down_interval(Duration::from_millis(10))
            .build();

        // The worker's internal `Arc<ThunkerInner>` keeps the inner alive
        // until the worker exits; we observe shutdown via `Arc::strong_count`
        // on a clone of the inner.
        let inner_probe = Arc::clone(&t.inner);
        drop(t);

        // Wait for the worker to observe shutdown and exit. Initial count
        // includes `inner_probe` + the worker; after shutdown only
        // `inner_probe` should remain.
        let mut last = Arc::strong_count(&inner_probe);
        for _ in 0..200 {
            last = Arc::strong_count(&inner_probe);
            if last == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(last, 1, "worker thread did not exit after shutdown (strong_count={last})");
    }

    /// Regression test for C1: when the last `Thunker` handle is dropped while
    /// items remain queued in the channel, the worker pool must drain and
    /// execute every remaining item before exiting. Otherwise the orphaned
    /// items would never invoke their shim's `mark_worker_done`, causing
    /// callers' `StackState::Drop` to spin forever.
    ///
    /// The test forces a queue backlog via a single-worker pool whose only
    /// worker is parked inside the first item; we then drop the `Thunker`,
    /// release the gate, and verify that every later-queued item ran.
    #[test]
    fn shutdown_drains_pending_items() {
        use std::sync::{Condvar, Mutex};

        type Gate = (Mutex<bool>, Condvar);

        fn park_until_released(ptr: *mut ()) {
            // SAFETY: ptr is a valid Arc<Gate> created via Arc::into_raw.
            let arc = unsafe { Arc::from_raw(ptr.cast::<Gate>()) };
            let (lock, cv) = &*arc;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = cv.wait(released).unwrap();
            }
        }

        fn bump(ptr: *mut ()) {
            // SAFETY: ptr is a valid Arc<AtomicUsize> created via Arc::into_raw.
            let arc = unsafe { Arc::from_raw(ptr.cast::<AtomicUsize>()) };
            let _ = arc.fetch_add(1, Ordering::SeqCst);
        }

        const PENDING: usize = 5;

        let t = Thunker::builder()
            .max_thread_count(1)
            .channel_capacity(16)
            .cool_down_interval(Duration::from_millis(10))
            .build();

        let gate: Arc<Gate> = Arc::new((Mutex::new(false), Condvar::new()));
        let gate_ptr = Arc::into_raw(Arc::clone(&gate)).cast_mut().cast::<()>();
        // SAFETY: `gate_ptr` is an `Arc<Gate>` raw pointer.
        t.send(unsafe { WorkItem::new(gate_ptr, park_until_released) });

        // Give the worker a moment to pick up and start parking on the gate.
        std::thread::sleep(Duration::from_millis(50));

        // Now queue several items that the (currently parked) worker has not
        // had a chance to receive. With max_thread_count=1 and the single
        // worker blocked on the gate, these MUST sit in the channel.
        let executed_count = Arc::new(AtomicUsize::new(0));
        for _ in 0..PENDING {
            let bump_arc = Arc::clone(&executed_count);
            let bump_ptr = Arc::into_raw(bump_arc).cast_mut().cast::<()>();
            // SAFETY: `bump_ptr` is an `Arc<AtomicUsize>` raw pointer.
            t.send(unsafe { WorkItem::new(bump_ptr, bump) });
        }

        // Drop the last `Thunker` handle — this sets `shutdown = true`. The
        // worker is currently inside `execute()` on the parked item; once we
        // release the gate it will loop back, observe shutdown, and (with
        // the C1 fix) drain the remaining PENDING items before exiting.
        let inner_probe = Arc::clone(&t.inner);
        drop(t);

        // Release the gate so the worker can finish item #1 and proceed to
        // the shutdown-drain path.
        {
            let (lock, cv) = &*gate;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }

        // Wait for the worker to exit (its Arc drops, leaving only inner_probe).
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while Arc::strong_count(&inner_probe) > 1 {
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not exit; strong_count={}",
                Arc::strong_count(&inner_probe)
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        // Every queued item must have been executed during the drain, not
        // silently dropped along with the channel.
        let got = executed_count.load(Ordering::SeqCst);
        assert_eq!(got, PENDING, "expected {PENDING} drained items, got {got}");
    }
}
