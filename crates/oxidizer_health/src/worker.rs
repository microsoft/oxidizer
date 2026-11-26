// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::future::{pending, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::task::{Poll, Waker};
use std::thread::{self, JoinHandle};
use std::{io, mem, ptr};

use foldhash::HashMap;
use performables::sync::channel::{WatchReceiver, WatchSender};
use tracing::{Level, event};

use crate::Health;
use crate::aggregator::ProcessAggregate;
use crate::component::{ComponentActor, ComponentShared};

pub(crate) enum Work {
    Register(u64, ComponentActor),
    Remove(u64, usize),
    Component(Weak<ComponentShared>),
    Stop,
}

/// Both process and component handles keep this worker alive.
pub(crate) struct Worker {
    pub(crate) sender: Sender<Work>,
    thread: Option<JoinHandle<()>>,
    status: Arc<Status>,
}

impl Worker {
    pub(crate) fn new(aggregate: ProcessAggregate) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel();
        let status = Arc::new(Status::new());
        let worker_status = Arc::clone(&status);
        let spawned = thread::Builder::new().name(String::from("oxidizer-health")).spawn(move || {
            // Disconnect all work queues before invoking terminal wakers, outside any unwind.
            let outcome = catch_unwind(AssertUnwindSafe(move || run(receiver, aggregate)));
            worker_status.stop();
            if let Err(cause) = outcome {
                resume_unwind(cause);
            }
        });
        Arc::new(Self::from_spawn(sender, status, spawned))
    }

    fn from_spawn(sender: Sender<Work>, status: Arc<Status>, spawned: io::Result<JoinHandle<()>>) -> Self {
        let thread = match spawned {
            Ok(thread) => Some(thread),
            Err(error) => {
                status.stop();
                event!(
                    name: "oxidizer_health.worker.start_failed",
                    Level::ERROR,
                    error.message = %error,
                    worker.name = "oxidizer-health",
                    "health worker {{worker.name}} failed to start: {{error.message}}"
                );
                None
            }
        };
        Self { sender, thread, status }
    }

    pub(crate) fn is_running(&self) -> bool {
        // Acquire: pairs with the Release store when the worker fails or stops.
        self.status.running.load(Ordering::Acquire)
    }

    pub(crate) fn subscribe(&self, updates: &WatchSender<()>) -> Changes {
        Changes(self.is_running().then(|| updates.subscribe()))
    }

    /// Requests shutdown and returns the worker thread if the caller can join it.
    fn request_stop(&mut self) -> Option<JoinHandle<()>> {
        let thread = self.thread.take()?;
        // A stopped worker has already reported its startup failure or panic.
        let _ = self.sender.send(Work::Stop);
        // A wake callback can release the last owner on the worker itself.
        (thread.thread().id() != thread::current().id()).then_some(thread)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(thread) = self.request_stop()
            && thread.join().map_err(discard_panic).is_err()
        {
            event!(
                name: "oxidizer_health.worker.terminated",
                Level::ERROR,
                worker.name = "oxidizer-health",
                "health worker {{worker.name}} terminated unexpectedly"
            );
        }
    }
}

/// Panic payloads are caller-owned values, even when recovered from a worker join during Drop.
fn discard_panic(cause: Box<dyn Any + Send>) {
    if let Err(nested) = catch_unwind(AssertUnwindSafe(|| drop(cause))) {
        // Release the destructor's payload too. If it panics again, do not recurse.
        mem::forget(catch_unwind(AssertUnwindSafe(|| drop(nested))));
    }
}

fn run(receiver: Receiver<Work>, mut aggregate: ProcessAggregate) {
    // Both pending registrations and registered actors belong to this thread, not to readers.
    let mut actors = HashMap::default();
    for work in receiver {
        match work {
            Work::Register(id, actor) => {
                actors.insert(id, actor);
                aggregate.set(id, Health::Unhealthy);
            }
            Work::Remove(id, blockers) => {
                actors.remove(&id);
                aggregate.remove(id, blockers);
            }
            Work::Component(component) => {
                if let Some(component) = component.upgrade().filter(|c| c.is_registered()) {
                    let actor = actors.get_mut(&component.id).expect("registration precedes component work");
                    component.process(actor, &mut aggregate);
                }
            }
            Work::Stop => return,
        }
    }
}

struct Status {
    running: AtomicBool,
    waiters: Mutex<HashMap<usize, Waker>>,
}

impl Status {
    fn new() -> Self {
        Self {
            running: AtomicBool::new(true),
            waiters: Mutex::default(),
        }
    }

    fn stop(&self) {
        // Release: every terminal observer must see fail-closed readiness before its wake.
        self.running.store(false, Ordering::Release);
        let waiters = mem::take(&mut *self.waiters.lock().unwrap_or_else(PoisonError::into_inner));
        for waker in waiters.into_values() {
            // Aggregation has ended. A secondary panic must not strand other waiters or replace
            // the worker failure that the caller of stop will resume.
            if let Err(cause) = catch_unwind(|| waker.wake()) {
                discard_panic(cause);
            }
        }
    }
}

/// Each handle can observe the worker's terminal transition once, independently of normal updates.
#[derive(Clone)]
pub(crate) struct Changes(Option<WatchReceiver<()>>);

impl Changes {
    pub(crate) async fn wait(&mut self, worker: &Worker) {
        let Some(updates) = &self.0 else { return pending().await };
        if Self::stopped_or_changed(updates, worker).await {
            self.0 = None;
        }
    }

    async fn stopped_or_changed(updates: &WatchReceiver<()>, worker: &Worker) -> bool {
        let mut changed = pin!(updates.changed_async());
        // A pending changed() holds its handle exclusively at a stable address until cancellation.
        let exit = ExitWaiter {
            status: &worker.status,
            key: ptr::from_ref(updates).addr(),
        };
        poll_fn(|cx| {
            // The regular sender lives as long as the handle; only a pending update needs a wake.
            let waker = changed.as_mut().poll(cx).is_pending().then(|| cx.waker().clone());
            // Only register while running. Clone and release caller wakers outside the lock.
            let _previous = {
                let mut waiters = exit.status.waiters.lock().unwrap_or_else(PoisonError::into_inner);
                if !worker.is_running() {
                    return Poll::Ready(true);
                }
                let Some(waker) = waker else { return Poll::Ready(false) };
                waiters.insert(exit.key, waker)
            };
            Poll::Pending
        })
        .await
    }
}

struct ExitWaiter<'a> {
    status: &'a Status,
    key: usize,
}

impl Drop for ExitWaiter<'_> {
    fn drop(&mut self) {
        let _discarded = self.status.waiters.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use std::panic::panic_any;
    use std::task::{Context, Wake};

    use futures::FutureExt;
    use performables::sync::channel::watch;
    use testing_aids::tracing_logs::Capture;

    use super::*;
    use crate::Component;
    use crate::component::ComponentOwner;

    #[test]
    fn shutdown_finishes_preceding_publications() {
        let mut worker = Arc::into_inner(Worker::new(ProcessAggregate::new(Weak::new()))).unwrap();
        let thread = worker.thread.take().unwrap();
        let worker = Arc::new(worker);
        assert!(worker.is_running());
        let (shared, actor) = ComponentShared::new(Arc::from("database"), 1, worker.sender.clone(), Arc::default());
        worker.sender.send(Work::Register(1, actor)).unwrap();
        let component = Component::new(ComponentOwner::new(shared, Arc::clone(&worker)));
        let mut publisher = component.publisher();
        publisher.publish(Health::Healthy, [("revision", 1)]);

        worker.sender.send(Work::Stop).unwrap();
        thread.join().unwrap();
        let report = futures::executor::block_on(component.report());
        assert_eq!(report.publisher_count(Health::Healthy), 1);
        assert_eq!(report.signals(Health::Healthy).next().unwrap().0, *publisher.signal());
        assert!(!worker.is_running());
    }

    #[test]
    fn change_cursor_delivers_separate_completed_turns() {
        let worker = Worker::new(ProcessAggregate::new(Weak::new()));
        let (updates, _) = watch(());
        let mut changes = worker.subscribe(&updates);
        for _ in 0..2 {
            updates.send_replace(());
            assert_eq!(changes.wait(&worker).now_or_never(), Some(()));
            assert!(changes.wait(&worker).now_or_never().is_none());
        }
    }

    #[test]
    #[cfg_attr(panic = "abort", ignore = "requires worker unwinding")]
    fn worker_exit_preserves_the_original_panic_across_terminal_callbacks() {
        struct Initial;
        struct Final;
        impl Drop for Final {
            fn drop(&mut self) {
                panic_any(());
            }
        }
        struct Secondary;
        impl Drop for Secondary {
            fn drop(&mut self) {
                panic_any(Final);
            }
        }
        struct Fail(Component);
        impl Wake for Fail {
            fn wake(self: Arc<Self>) {
                if self.0.state() == Health::Healthy {
                    panic_any(Initial);
                }
                panic_any(Secondary);
            }
        }
        let mut worker = Arc::into_inner(Worker::new(ProcessAggregate::new(Weak::new()))).unwrap();
        let thread = worker.thread.take().unwrap();
        let worker = Arc::new(worker);
        let (shared, actor) = ComponentShared::new(Arc::from("database"), 1, worker.sender.clone(), Arc::default());
        worker.sender.send(Work::Register(1, actor)).unwrap();
        let mut component = Component::new(ComponentOwner::new(shared, Arc::clone(&worker)));
        let owner = component.clone();
        let waker = Waker::from(Arc::new(Fail(owner.clone())));
        let mut changed = pin!(component.changed());
        assert!(changed.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        let _publisher = owner.publisher();
        // The expected callback panic may already have disconnected the worker.
        let _ = worker.sender.send(Work::Stop);
        assert!(thread.join().unwrap_err().is::<Initial>());
    }

    #[test]
    fn spawn_failure_marks_worker_unavailable_until_shutdown() {
        let (sender, receiver) = mpsc::channel();
        let worker = Worker::from_spawn(sender, Arc::new(Status::new()), Err(io::ErrorKind::WouldBlock.into()));
        assert!(!worker.is_running());
        drop(worker);
        assert!(matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected)));
    }

    #[test]
    fn request_stop_sends_stop_and_returns_the_worker_thread() {
        let (sender, receiver) = mpsc::channel();
        let spawned = thread::spawn(|| {});
        let mut worker = Worker::from_spawn(sender, Arc::new(Status::new()), Ok(spawned));

        let thread = worker.request_stop().unwrap();

        assert!(matches!(receiver.try_recv(), Ok(Work::Stop)));
        thread.join().unwrap();
    }

    #[test]
    fn shutdown_joins_the_worker_and_reports_a_failed_join() {
        struct FailedJoin;
        impl Drop for FailedJoin {
            fn drop(&mut self) {
                panic!("failed worker payload panicked during cleanup");
            }
        }
        for &fail in if cfg!(panic = "unwind") { &[false, true][..] } else { &[false][..] } {
            let capture = Capture::new();
            let _guard = tracing::subscriber::set_default(capture.subscriber());
            let (sender, _receiver) = mpsc::channel();
            let spawned = thread::spawn(move || {
                if fail {
                    panic_any(FailedJoin);
                }
            });
            drop(Worker::from_spawn(sender, Arc::new(Status::new()), Ok(spawned)));
            assert_eq!(capture.output().contains("terminated unexpectedly"), fail);
        }
    }

    #[test]
    fn shutdown_on_the_worker_requests_stop_without_returning_a_self_join() {
        let (sender, receiver) = mpsc::channel();
        let (owner, release) = mpsc::channel::<Worker>();
        let (done, finished) = mpsc::channel();
        let spawned = thread::spawn(move || {
            let mut worker = release.recv().unwrap();
            assert!(worker.request_stop().is_none());
            assert!(matches!(receiver.try_recv(), Ok(Work::Stop)));
            done.send(()).unwrap();
        });
        owner
            .send(Worker::from_spawn(sender, Arc::new(Status::new()), Ok(spawned)))
            .unwrap();
        finished.recv().unwrap();
    }
}
