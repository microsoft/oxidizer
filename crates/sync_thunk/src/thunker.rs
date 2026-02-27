// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crossfire::mpmc;

use crate::{ThunkerBuilder, WorkItem};

/// Shared state between the `Thunker` handle and its worker threads.
struct ThunkerInner {
    sender: crossfire::MTx<mpmc::Array<WorkItem>>,
    receiver: crossfire::MRx<mpmc::Array<WorkItem>>,
    thread_count: AtomicUsize,
    pending_count: AtomicUsize,
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
#[derive(Clone)]
pub struct Thunker {
    inner: Arc<ThunkerInner>,
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
        let (sender, receiver) = mpmc::bounded_blocking(builder.channel_capacity);
        let thunker = Self {
            inner: Arc::new(ThunkerInner {
                sender,
                receiver,
                thread_count: AtomicUsize::new(0),
                pending_count: AtomicUsize::new(0),
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
        let prev_pending = self.inner.pending_count.fetch_add(1, Ordering::Relaxed);
        let threads = self.inner.thread_count.load(Ordering::Acquire);

        // Scale up if the queue is backing up and we haven't hit the limit.
        if prev_pending >= threads
            && threads < self.inner.max_thread_count
            && self
                .inner
                .thread_count
                .compare_exchange(threads, threads + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            Self::spawn_worker_already_counted(&self.inner);
        }

        self.inner.sender.send(item).expect("channel is closed");
    }

    /// Spawns a worker thread and increments the thread count.
    fn spawn_worker(inner: &Arc<ThunkerInner>) {
        let _ = inner.thread_count.fetch_add(1, Ordering::AcqRel);
        Self::spawn_worker_already_counted(inner);
    }

    /// Spawns a worker thread, assuming the caller already incremented the count.
    fn spawn_worker_already_counted(inner: &Arc<ThunkerInner>) {
        let inner = Arc::clone(inner);
        std::thread::Builder::new()
            .name("sync-thunk-worker".into())
            .spawn(move || {
                Self::worker_loop(&inner);
            })
            .expect("OS refused to spawn a sync-thunk worker thread; the system may be out of resources");
    }

    fn worker_loop(inner: &ThunkerInner) {
        loop {
            match inner.receiver.recv_timeout(inner.cool_down_interval) {
                Ok(item) => {
                    // Decrement pending_count even if the work item panics.
                    struct DecrementOnDrop<'a>(&'a AtomicUsize);
                    impl Drop for DecrementOnDrop<'_> {
                        fn drop(&mut self) {
                            let _ = self.0.fetch_sub(1, Ordering::Relaxed);
                        }
                    }
                    let _guard = DecrementOnDrop(&inner.pending_count);
                    item.execute();
                }
                Err(crossfire::RecvTimeoutError::Timeout) => {
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
                            Ok(_) => return,
                            Err(actual) => count = actual,
                        }
                    }
                    // Last worker — keep running.
                }
                Err(crossfire::RecvTimeoutError::Disconnected) => {
                    let _ = inner.thread_count.fetch_sub(1, Ordering::AcqRel);
                    return;
                }
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
            .field("pending_count", &self.inner.pending_count.load(Ordering::Relaxed))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

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
        let item = WorkItem::new(flag_ptr, set_flag);
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
            t.send(WorkItem::new(b_ptr, wait_on_barrier));
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
}
