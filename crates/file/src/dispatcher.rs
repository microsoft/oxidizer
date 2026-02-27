// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::pin::Pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Context, Poll};
use core::time::Duration;
use std::fmt;
use std::sync::Arc;

const MAX_THREADS: usize = 4;
const IDLE_TIMEOUT: Duration = Duration::from_secs(10);

struct DispatcherInner {
    sender: flume::Sender<async_task::Runnable>,
    receiver: flume::Receiver<async_task::Runnable>,
    thread_count: AtomicUsize,
    pending_count: AtomicUsize,
}

/// A thread pool that executes blocking filesystem operations on behalf
/// of the async API.
///
/// Starts with a single worker thread and scales up to [`MAX_THREADS`]
/// when the pending-operation count exceeds the current thread count.
/// Idle workers scale back down after [`IDLE_TIMEOUT`].
#[derive(Clone)]
pub struct Dispatcher {
    inner: Arc<DispatcherInner>,
}

impl Dispatcher {
    /// Creates a new dispatcher with one initial worker thread.
    pub fn new() -> Self {
        let (sender, receiver) = flume::unbounded();
        let dispatcher = Self {
            inner: Arc::new(DispatcherInner {
                sender,
                receiver,
                thread_count: AtomicUsize::new(0),
                pending_count: AtomicUsize::new(0),
            }),
        };
        Self::spawn_worker(&dispatcher.inner);
        dispatcher
    }

    /// Dispatches a blocking operation to a worker thread.
    ///
    /// Returns a future that resolves to the operation's return value.
    /// If the closure panics, the panic is forwarded to the caller.
    pub fn dispatch<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> DispatchFuture<T> {
        let sender = self.inner.sender.clone();
        let schedule = move |runnable: async_task::Runnable| {
            let _ = sender.send(runnable);
        };

        let (runnable, task) = async_task::spawn(async move { std::panic::catch_unwind(core::panic::AssertUnwindSafe(f)) }, schedule);

        let prev_pending = self.inner.pending_count.fetch_add(1, Ordering::Relaxed);
        let threads = self.inner.thread_count.load(Ordering::Acquire);

        // Scale up if the queue is backing up and we haven't hit the limit.
        if prev_pending >= threads
            && threads < MAX_THREADS
            && self
                .inner
                .thread_count
                .compare_exchange(threads, threads + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            Self::spawn_worker_already_counted(&self.inner);
        }

        runnable.schedule();

        DispatchFuture { task }
    }

    /// Dispatches a blocking operation that may borrow from the caller.
    ///
    /// Like [`dispatch`](Self::dispatch), but the returned future **blocks on
    /// drop** if the closure has not yet completed. This guarantees that
    /// data borrowed via raw pointers in the closure remains valid for the
    /// closure's entire execution, even when the future is cancelled.
    ///
    /// # Safety
    ///
    /// The closure may capture raw pointers to caller-owned data. The caller
    /// must ensure those pointers are derived from data that lives at least
    /// until the returned [`ScopedDispatchFuture`] is dropped.
    pub fn dispatch_scoped<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> ScopedDispatchFuture<T> {
        let (done_tx, done_rx) = flume::bounded(1);
        let signal = SignalOnDrop(Some(done_tx));

        let sender = self.inner.sender.clone();
        let schedule = move |runnable: async_task::Runnable| {
            let _ = sender.send(runnable);
        };

        let (runnable, task) = async_task::spawn(
            async move {
                let result = std::panic::catch_unwind(core::panic::AssertUnwindSafe(f));
                drop(signal);
                result
            },
            schedule,
        );

        let prev_pending = self.inner.pending_count.fetch_add(1, Ordering::Relaxed);
        let threads = self.inner.thread_count.load(Ordering::Acquire);

        if prev_pending >= threads
            && threads < MAX_THREADS
            && self
                .inner
                .thread_count
                .compare_exchange(threads, threads + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            Self::spawn_worker_already_counted(&self.inner);
        }

        runnable.schedule();

        ScopedDispatchFuture { task, done_rx }
    }

    /// Spawns a worker thread and increments the thread count.
    fn spawn_worker(inner: &Arc<DispatcherInner>) {
        let _ = inner.thread_count.fetch_add(1, Ordering::AcqRel);
        Self::spawn_worker_already_counted(inner);
    }

    /// Spawns a worker thread, assuming the caller already incremented the count.
    fn spawn_worker_already_counted(inner: &Arc<DispatcherInner>) {
        let inner = Arc::clone(inner);
        let _ = std::thread::Builder::new()
            .name("file-dispatcher".into())
            .spawn(move || {
                Self::worker_loop(&inner);
            })
            .expect("failed to spawn dispatcher worker thread");
    }

    fn worker_loop(inner: &DispatcherInner) {
        loop {
            match inner.receiver.recv_timeout(IDLE_TIMEOUT) {
                Ok(runnable) => {
                    let _ = runnable.run();
                    let _ = inner.pending_count.fetch_sub(1, Ordering::Relaxed);
                }
                Err(flume::RecvTimeoutError::Timeout) => {
                    // Scale down: CAS ensures at least one worker remains.
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
                Err(flume::RecvTimeoutError::Disconnected) => {
                    let _ = inner.thread_count.fetch_sub(1, Ordering::AcqRel);
                    return;
                }
            }
        }
    }
}

impl fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dispatcher")
            .field("threads", &self.inner.thread_count.load(Ordering::Relaxed))
            .field("pending", &self.inner.pending_count.load(Ordering::Relaxed))
            .finish()
    }
}

/// A future that resolves to the result of a dispatched operation.
///
/// If the worker thread panics, the original panic is re-raised on the
/// calling task via [`std::panic::resume_unwind`].
pub struct DispatchFuture<T> {
    task: async_task::Task<std::thread::Result<T>>,
}

impl<T> Future for DispatchFuture<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let this = self.get_mut();
        match Pin::new(&mut this.task).poll(cx) {
            Poll::Ready(Ok(value)) => Poll::Ready(value),
            Poll::Ready(Err(payload)) => {
                // Re-raise the original panic from the worker thread.
                std::panic::resume_unwind(payload);
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Sends a completion signal when dropped, whether the closure completed
/// normally or the task was cancelled before it ran.
struct SignalOnDrop(Option<flume::Sender<()>>);

impl Drop for SignalOnDrop {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

/// A cancellation-safe dispatch future for operations that borrow from the
/// caller via raw pointers.
///
/// If this future is dropped before the dispatched closure completes, the
/// destructor **blocks the current thread** until the closure finishes. This
/// guarantees that any caller-owned data referenced by raw pointers in the
/// closure remains valid for the closure's entire execution.
///
/// In the normal (non-cancelled) path, the future resolves asynchronously
/// with zero blocking.
pub struct ScopedDispatchFuture<T> {
    task: async_task::Task<std::thread::Result<T>>,
    done_rx: flume::Receiver<()>,
}

impl<T> Future for ScopedDispatchFuture<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let this = self.get_mut();
        match Pin::new(&mut this.task).poll(cx) {
            Poll::Ready(Ok(value)) => {
                // Drain the signal so Drop doesn't block.
                let _ = this.done_rx.try_recv();
                Poll::Ready(value)
            }
            Poll::Ready(Err(payload)) => {
                let _ = this.done_rx.try_recv();
                std::panic::resume_unwind(payload);
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> Drop for ScopedDispatchFuture<T> {
    fn drop(&mut self) {
        // Block until the closure signals completion (or confirms it was
        // never started). This is the cancellation-safety guarantee: the
        // caller's borrowed data cannot be freed until the worker is done.
        let _ = self.done_rx.recv();
    }
}
