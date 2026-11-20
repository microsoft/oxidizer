// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime hook contracts.

#![cfg(feature = "rt")]
#![allow(clippy::panic, reason = "tests deliberately trigger panics")]
#![allow(clippy::unwrap_used, reason = "test code")]

mod support;

use std::cell::Cell;
use std::collections::HashSet;
use std::future::poll_fn;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::Poll;
use std::thread::{self, ThreadId};
use std::time::Duration;

use arty::core::{Thread, ThreadAware};
use arty::runtime::hook::{HookFn, TaskObserver, TaskPanic, TaskSpawn, WorkerSpawn};
use arty::runtime::policy::WorkersPolicy;
use arty::runtime::{Runtime, RuntimeControl};
use arty::task::Context;
use support::JoinHandleExt as _;

#[derive(Clone)]
struct WorkerSpawnProbe {
    destination: Option<ThreadId>,
    observed: mpsc::Sender<WorkerSpawnObservation>,
}

impl ThreadAware for WorkerSpawnProbe {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.destination = Some(destination.id());
    }
}

struct WorkerSpawnObservation {
    callback: ThreadId,
    event: ThreadId,
    relocated: ThreadId,
}

#[test]
fn worker_spawn_runs_once_per_worker() {
    let (observed, observations) = mpsc::channel();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .on(HookFn::<WorkerSpawn, _>::new(
            WorkerSpawnProbe {
                destination: None,
                observed,
            },
            |event, data| {
                data.observed
                    .send(WorkerSpawnObservation {
                        callback: std::thread::current().id(),
                        event: event.thread().id(),
                        relocated: data.destination.unwrap(),
                    })
                    .unwrap();
            },
        ))
        .build()
        .unwrap();

    let observations = (0..2).map(|_| observations.try_recv().unwrap()).collect::<Vec<_>>();
    assert_eq!(observations.len(), 2);
    assert_ne!(observations[0].event, observations[1].event);
    for observation in observations {
        assert_eq!(observation.callback, observation.event);
        assert_eq!(observation.relocated, observation.event);
    }
    runtime.stop().unwrap();
}

#[test]
fn builder_retains_worker_spawn_data_until_shutdown() {
    let calls = Arc::new(AtomicUsize::new(0));
    let lifetime = Arc::downgrade(&calls);
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .on(HookFn::<WorkerSpawn, _>::new(PanicCount(calls), |_event, data| {
            data.0.fetch_add(1, Ordering::SeqCst);
        }))
        .build()
        .unwrap();

    assert_eq!(lifetime.upgrade().unwrap().load(Ordering::SeqCst), 2);
    runtime.stop().unwrap();
    assert!(lifetime.upgrade().is_none());
}

#[test]
fn worker_local_hooks_accept_non_sync_data() {
    let worker_spawn_calls = Arc::new(AtomicUsize::new(0));
    let task_panic_calls = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<WorkerSpawn, _>::new(
            WorkerLocalData {
                calls: Arc::clone(&worker_spawn_calls),
                local_calls: Cell::new(0),
            },
            |_event, data| {
                data.local_calls.set(data.local_calls.get() + 1);
                data.calls.fetch_add(1, Ordering::SeqCst);
            },
        ))
        .on(HookFn::<TaskPanic, _>::new(
            WorkerLocalData {
                calls: Arc::clone(&task_panic_calls),
                local_calls: Cell::new(0),
            },
            |event, data| {
                data.local_calls.set(data.local_calls.get() + 1);
                data.calls.fetch_add(1, Ordering::SeqCst);
                event.set_ignore();
            },
        ))
        .build()
        .unwrap();

    assert_eq!(worker_spawn_calls.load(Ordering::SeqCst), 1);
    assert!(
        runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| async { panic!("task panic") })
            .join()
            .unwrap_err()
            .is_panic()
    );
    assert_eq!(task_panic_calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 }).join().unwrap(), 42);
    runtime.stop().unwrap();
}

#[derive(Clone)]
struct SharedLog(Arc<Mutex<Vec<&'static str>>>);

impl SharedLog {
    fn push(&self, value: &'static str) {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(value);
    }

    fn take(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }
}

impl ThreadAware for SharedLog {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[derive(Clone)]
struct SpawnSourceProbe {
    events: mpsc::Sender<(Option<ThreadId>, ThreadId)>,
    observers: mpsc::Sender<(Option<ThreadId>, ThreadId, ThreadId)>,
}

impl ThreadAware for SpawnSourceProbe {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

struct SpawnSourceObserver {
    source: Option<ThreadId>,
    destination: Option<ThreadId>,
    observations: mpsc::Sender<(Option<ThreadId>, ThreadId, ThreadId)>,
}

impl ThreadAware for SpawnSourceObserver {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.destination = Some(destination.id());
    }
}

impl TaskObserver for SpawnSourceObserver {
    fn poll_start(&mut self) {
        self.observations
            .send((self.source, self.destination.unwrap(), thread::current().id()))
            .unwrap();
    }

    fn poll_end(&mut self, _outcome: Poll<()>) {}
}

#[derive(Default)]
struct SpawnLifecycle {
    callback: Option<ThreadId>,
    source: Option<ThreadId>,
    target: Option<ThreadId>,
    relocation: Option<(Option<ThreadId>, ThreadId)>,
    poll_starts: Vec<ThreadId>,
    poll_ends: Vec<(ThreadId, Poll<()>)>,
}

#[derive(Clone)]
struct SpawnLifecycleProbe {
    enabled: Arc<AtomicBool>,
    lifecycle: Arc<Mutex<SpawnLifecycle>>,
}

impl ThreadAware for SpawnLifecycleProbe {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

struct LifecyclePoll(Arc<Mutex<SpawnLifecycle>>);

impl ThreadAware for LifecyclePoll {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.0.lock().unwrap().relocation = Some((source.map(Thread::id), destination.id()));
    }
}

impl TaskObserver for LifecyclePoll {
    fn poll_start(&mut self) {
        self.0.lock().unwrap().poll_starts.push(thread::current().id());
    }

    fn poll_end(&mut self, outcome: Poll<()>) {
        self.0.lock().unwrap().poll_ends.push((thread::current().id(), outcome));
    }
}

struct PollRecorder {
    before: &'static str,
    after: &'static str,
    outcome: Poll<()>,
    destination: Option<ThreadId>,
    log: SharedLog,
}

impl ThreadAware for PollRecorder {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.destination = Some(destination.id());
    }
}

impl TaskObserver for PollRecorder {
    fn poll_start(&mut self) {
        assert_eq!(self.destination, Some(thread::current().id()));
        self.log.push(self.before);
    }

    fn poll_end(&mut self, outcome: Poll<()>) {
        assert_eq!(outcome, self.outcome);
        self.log.push(self.after);
    }
}

#[test]
fn spawn_hook_brackets_the_complete_task_poll_in_registration_order() {
    let log = SharedLog(Arc::new(Mutex::new(Vec::new())));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<TaskSpawn, _>::new(log.clone(), |event, log| {
            log.push("outer spawn");
            event.observe_with(
                PollRecorder {
                    before: "outer before",
                    after: "outer after",
                    outcome: Poll::Ready(()),
                    destination: None,
                    log: log.clone(),
                },
                std::convert::identity,
            );
        }))
        .on(HookFn::<TaskSpawn, _>::new(log.clone(), |event, log| {
            log.push("inner spawn");
            event.observe_with(
                PollRecorder {
                    before: "inner before",
                    after: "inner after",
                    outcome: Poll::Ready(()),
                    destination: None,
                    log: log.clone(),
                },
                std::convert::identity,
            );
        }))
        .build()
        .unwrap();

    runtime
        .scheduler()
        .spawn_anywhere(log.clone(), |_, task_log| async move {
            task_log.push("task");
        })
        .join()
        .unwrap();

    assert_eq!(
        log.take(),
        [
            "outer spawn",
            "inner spawn",
            "outer before",
            "inner before",
            "task",
            "inner after",
            "outer after",
        ]
    );
    runtime.stop().unwrap();
}

#[test]
fn poll_hook_scope_completes_when_the_task_panics() {
    let log = SharedLog(Arc::new(Mutex::new(Vec::new())));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(TaskPanic::ignore())
        .on(HookFn::<TaskSpawn, _>::new(log.clone(), |event, log| {
            event.observe_with(
                PollRecorder {
                    before: "before",
                    after: "after",
                    outcome: Poll::Ready(()),
                    destination: None,
                    log: log.clone(),
                },
                std::convert::identity,
            );
        }))
        .build()
        .unwrap();

    let task = runtime.scheduler().spawn_anywhere((), |_, ()| async move {
        panic!("task panic");
    });

    assert!(task.join().unwrap_err().is_panic());
    assert_eq!(log.take(), ["before", "after"]);
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 }).join().unwrap(), 42);
    runtime.stop().unwrap();
}

struct PanickingPoll;

impl ThreadAware for PanickingPoll {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl TaskObserver for PanickingPoll {
    fn poll_start(&mut self) {
        panic!("poll hook panic");
    }

    fn poll_end(&mut self, _outcome: Poll<()>) {}
}

#[test]
fn poll_hook_panic_fails_the_worker_instead_of_the_task() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<TaskSpawn, _>::new((), |event, ()| {
            event.observe_with(PanickingPoll, std::convert::identity);
        }))
        .build()
        .unwrap();

    let error = runtime.scheduler().spawn_anywhere((), |_, ()| async {}).join().unwrap_err();

    assert!(error.is_shutdown());
    assert!(!error.is_panic());
    assert!(runtime.stop().is_err());
}

thread_local! {
    static POLL_SCOPE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

struct ScopedPoll;

impl ThreadAware for ScopedPoll {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl TaskObserver for ScopedPoll {
    fn poll_start(&mut self) {
        POLL_SCOPE_DEPTH.with(|depth| {
            assert_eq!(depth.replace(1), 0);
        });
    }

    fn poll_end(&mut self, _outcome: Poll<()>) {
        POLL_SCOPE_DEPTH.set(0);
    }
}

#[derive(Clone)]
struct PollOutcomes(Arc<Mutex<Vec<Poll<()>>>>);

impl PollOutcomes {
    fn take(&self) -> Vec<Poll<()>> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

impl ThreadAware for PollOutcomes {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl TaskObserver for PollOutcomes {
    fn poll_start(&mut self) {}

    fn poll_end(&mut self, outcome: Poll<()>) {
        self.0.lock().unwrap().push(outcome);
    }
}

#[test]
fn task_observer_receives_each_poll_outcome() {
    let outcomes = PollOutcomes(Arc::new(Mutex::new(Vec::new())));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<TaskSpawn, _>::new(outcomes.clone(), |event, outcomes| {
            event.observe_with(outcomes.clone(), std::convert::identity);
        }))
        .build()
        .unwrap();

    runtime
        .scheduler()
        .spawn_anywhere((), |_, ()| async {
            let mut first = true;
            poll_fn(move |cx| {
                if first {
                    first = false;
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
        })
        .join()
        .unwrap();

    assert_eq!(outcomes.take(), [Poll::Pending, Poll::Ready(())]);
    runtime.stop().unwrap();
}

#[test]
fn poll_hook_scope_is_removed_before_the_next_poll() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<TaskSpawn, _>::new((), |event, ()| {
            event.observe_with(ScopedPoll, std::convert::identity);
        }))
        .build()
        .unwrap();

    let task = runtime.scheduler().spawn_anywhere((), |_, ()| async {
        let mut first = true;
        poll_fn(move |cx| {
            POLL_SCOPE_DEPTH.with(|depth| assert_eq!(depth.get(), 1));
            if first {
                first = false;
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    });

    task.join().unwrap();
    runtime.stop().unwrap();
}

#[test]
fn spawn_hook_covers_runtime_local_relocating_and_specific_worker_submissions() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .on(HookFn::<TaskSpawn, _>::new(PanicCount(Arc::clone(&calls)), |event, count| {
            assert!(event.origin().is_none());
            count.0.fetch_add(1, Ordering::SeqCst);
        }))
        .build()
        .unwrap();
    runtime.scheduler().spawn_anywhere((), |_, ()| async {}).join().unwrap();
    let (control, scheduler) = runtime
        .scheduler()
        .block_on(async |cx| (cx.control().clone(), cx.scheduler().clone()))
        .unwrap();
    let baseline = calls.load(Ordering::SeqCst);
    let destination = control.workers().into_iter().next().unwrap();

    scheduler.spawn(async |_| {}).join().unwrap();
    scheduler.spawn_anywhere((), |()| async {}).join().unwrap();
    scheduler.spawn_at(destination, (), |()| async {}).join().unwrap();

    assert_eq!(calls.load(Ordering::SeqCst) - baseline, 3);
    runtime.stop().unwrap();
}

#[test]
fn spawn_hook_uses_the_actual_submitting_worker() {
    let (observed, observations) = mpsc::channel();
    let (observed_observer, observer_observations) = mpsc::channel();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .on(HookFn::<TaskSpawn, _>::new(
            SpawnSourceProbe {
                events: observed,
                observers: observed_observer,
            },
            |event, probe| {
                let source = event.origin().map(Thread::id);
                probe.events.send((source, event.target().id())).unwrap();
                event.observe_with(
                    SpawnSourceObserver {
                        source,
                        destination: None,
                        observations: probe.observers.clone(),
                    },
                    std::convert::identity,
                );
            },
        ))
        .build()
        .unwrap();
    let (scheduler, destination, target) = runtime
        .scheduler()
        .block_on(async |cx| {
            let workers = cx.control().workers();
            let destination = workers.into_iter().find(|worker| worker.id() != cx.thread().id()).unwrap().clone();
            let target = workers.into_iter().find(|worker| worker.id() == cx.thread().id()).unwrap().clone();
            (cx.scheduler().clone(), destination, target)
        })
        .unwrap();
    for _ in observations.try_iter() {}
    for _ in observer_observations.try_iter() {}

    let bound = scheduler.clone();
    let other_worker = target.clone();
    scheduler
        .spawn_at(&destination, (bound, other_worker), |(bound, other_worker)| async move {
            bound.spawn_at(&other_worker, (), |()| async {}).await.unwrap();
        })
        .join()
        .unwrap();

    let submissions: Vec<_> = (0..2).map(|_| observations.recv_timeout(Duration::from_secs(5)).unwrap()).collect();
    assert!(submissions.contains(&(None, destination.id())));
    assert!(submissions.contains(&(Some(destination.id()), target.id())));
    let relocated: Vec<_> = (0..2)
        .map(|_| observer_observations.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    assert!(relocated.contains(&(None, destination.id(), destination.id())));
    assert!(relocated.contains(&(Some(destination.id()), target.id(), target.id())));
    runtime.stop().unwrap();
}

#[test]
fn worker_originated_spawn_runs_the_complete_hook_lifecycle() {
    let enabled = Arc::new(AtomicBool::new(false));
    let lifecycle = Arc::new(Mutex::new(SpawnLifecycle::default()));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .on(HookFn::<TaskSpawn, _>::new(
            SpawnLifecycleProbe {
                enabled: Arc::clone(&enabled),
                lifecycle: Arc::clone(&lifecycle),
            },
            |event, probe| {
                if !probe.enabled.load(Ordering::Acquire) {
                    return;
                }
                {
                    let mut lifecycle = probe.lifecycle.lock().unwrap();
                    lifecycle.callback = Some(thread::current().id());
                    lifecycle.source = event.origin().map(Thread::id);
                    lifecycle.target = Some(event.target().id());
                }
                event.observe_with(LifecyclePoll(Arc::clone(&probe.lifecycle)), std::convert::identity);
            },
        ))
        .build()
        .unwrap();
    let (scheduler, source, destination) = runtime
        .scheduler()
        .block_on(async |cx| {
            let destination = cx
                .control()
                .workers()
                .into_iter()
                .find(|worker| worker.id() != cx.thread().id())
                .unwrap()
                .clone();
            (cx.scheduler().clone(), cx.thread().clone(), destination)
        })
        .unwrap();

    let bound = scheduler.clone();
    let probe = SpawnLifecycleProbe {
        enabled: Arc::clone(&enabled),
        lifecycle: Arc::clone(&lifecycle),
    };
    scheduler
        .spawn_at(
            &source,
            (bound, destination.clone(), probe),
            |(bound, destination, probe)| async move {
                probe.enabled.store(true, Ordering::Release);
                bound.spawn_at(&destination, (), |()| async {}).await.unwrap();
                probe.enabled.store(false, Ordering::Release);
            },
        )
        .join()
        .unwrap();

    let lifecycle = lifecycle.lock().unwrap();
    assert_eq!(
        (
            lifecycle.callback,
            lifecycle.source,
            lifecycle.target,
            lifecycle.relocation,
            lifecycle.poll_starts.as_slice(),
            lifecycle.poll_ends.as_slice(),
        ),
        (
            Some(source.id()),
            Some(source.id()),
            Some(destination.id()),
            Some((Some(source.id()), destination.id())),
            [destination.id()].as_slice(),
            [(destination.id(), Poll::Ready(()))].as_slice(),
        )
    );
    runtime.stop().unwrap();
}

#[test]
fn spawn_hook_runs_for_a_detached_scheduler() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<TaskSpawn, _>::new(PanicCount(Arc::clone(&calls)), |_, count| {
            count.0.fetch_add(1, Ordering::SeqCst);
        }))
        .build()
        .unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let baseline = calls.load(Ordering::SeqCst);

    let observed = scheduler
        .spawn({
            let calls = Arc::clone(&calls);
            move |_| async move { calls.load(Ordering::SeqCst) }
        })
        .join()
        .unwrap();

    assert_eq!(observed, baseline + 1);
    assert_eq!(calls.load(Ordering::SeqCst), baseline + 1);
    runtime.stop().unwrap();
}

#[derive(Clone)]
struct NeutralSpawnProbe {
    factory_created: Arc<AtomicBool>,
    relocation: ObserverRelocation,
    began: Arc<AtomicBool>,
}

type ObserverRelocation = Arc<Mutex<Option<(Option<ThreadId>, ThreadId)>>>;

impl ThreadAware for NeutralSpawnProbe {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

struct RelocatedPoll {
    factory_created: Arc<AtomicBool>,
    relocation: ObserverRelocation,
    began: Arc<AtomicBool>,
}

impl ThreadAware for RelocatedPoll {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        assert!(!self.factory_created.load(Ordering::Acquire));
        self.relocation.lock().unwrap().replace((source.map(Thread::id), destination.id()));
    }
}

impl TaskObserver for RelocatedPoll {
    fn poll_start(&mut self) {
        assert!(!self.factory_created.load(Ordering::Acquire));
        let destination = self.relocation.lock().unwrap().unwrap().1;
        assert_eq!(destination, thread::current().id());
        self.began.store(true, Ordering::Release);
    }

    fn poll_end(&mut self, _outcome: Poll<()>) {}
}

#[derive(Clone)]
struct TargetTaskData {
    factory_created: Arc<AtomicBool>,
    began: Arc<AtomicBool>,
}

impl ThreadAware for TargetTaskData {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[test]
fn detached_scheduler_relocates_and_starts_observer_before_factory() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let factory_created = Arc::new(AtomicBool::new(false));
    let relocation = Arc::new(Mutex::new(None));
    let began = Arc::new(AtomicBool::new(false));
    let (scheduler, destination) = runtime
        .scheduler()
        .block_on(async |cx| {
            let source = cx.thread().id();
            let destination = cx
                .control()
                .workers()
                .into_iter()
                .find(|worker| worker.id() != source)
                .unwrap()
                .clone();
            cx.control().on(HookFn::<TaskSpawn, _>::new(
                NeutralSpawnProbe {
                    factory_created: Arc::clone(&factory_created),
                    relocation: Arc::clone(&relocation),
                    began: Arc::clone(&began),
                },
                |event, probe| {
                    assert!(event.origin().is_none());
                    probe.factory_created.store(false, Ordering::Release);
                    event.observe_with(
                        RelocatedPoll {
                            factory_created: Arc::clone(&probe.factory_created),
                            relocation: Arc::clone(&probe.relocation),
                            began: Arc::clone(&probe.began),
                        },
                        std::convert::identity,
                    );
                },
            ));
            (cx.scheduler().clone(), destination)
        })
        .unwrap();

    scheduler
        .spawn_at(
            &destination,
            TargetTaskData {
                factory_created: Arc::clone(&factory_created),
                began: Arc::clone(&began),
            },
            |data| {
                data.factory_created.store(true, Ordering::Release);
                async move {
                    assert!(data.began.load(Ordering::Acquire));
                }
            },
        )
        .join()
        .unwrap();

    assert_eq!(*relocation.lock().unwrap(), Some((None, destination.id())));
    runtime.stop().unwrap();
}

#[derive(Clone)]
struct LateSpawnProbe {
    calls: Arc<AtomicUsize>,
    destinations: Arc<Mutex<Vec<ThreadId>>>,
}

impl ThreadAware for LateSpawnProbe {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.destinations.lock().unwrap().push(destination.id());
    }
}

#[test]
fn control_registration_precedes_later_task_submissions() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let (control, scheduler) = runtime
        .scheduler()
        .block_on(async |cx| (cx.control().clone(), cx.scheduler().clone()))
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let destinations = Arc::new(Mutex::new(Vec::new()));

    control.on(HookFn::<TaskSpawn, _>::new(
        LateSpawnProbe {
            calls: Arc::clone(&calls),
            destinations: Arc::clone(&destinations),
        },
        |event, probe| {
            assert!(event.origin().is_none());
            probe.calls.fetch_add(1, Ordering::SeqCst);
        },
    ));
    let (initialized, initialization) = mpsc::channel();
    control.on(HookFn::<WorkerSpawn, _>::new(
        InitSignal {
            calls: PanicCount(Arc::new(AtomicUsize::new(0))),
            initialized,
        },
        |_event, data| {
            data.initialized.send(()).unwrap();
        },
    ));
    for _ in control.workers() {
        initialization.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    let tasks: Vec<_> = control
        .workers()
        .into_iter()
        .map(|worker| scheduler.spawn_at(worker, (), |()| async {}))
        .collect();
    for task in tasks {
        task.join().unwrap();
    }

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(destinations.lock().unwrap().iter().copied().collect::<HashSet<_>>().len(), 2);

    control.shutdown();
    assert!(scheduler.spawn(async |_| {}).join().unwrap_err().is_shutdown());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    runtime.stop().unwrap();
}

#[test]
fn spawn_hook_registration_applies_before_local_spawn() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));

    runtime
        .scheduler()
        .block_on(async |cx| {
            cx.control()
                .on(HookFn::<TaskSpawn, _>::new(PanicCount(Arc::clone(&calls)), |_, count| {
                    count.0.fetch_add(1, Ordering::SeqCst);
                }));
            cx.scheduler().spawn(async |_| {}).await.unwrap();
        })
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    runtime.stop().unwrap();
}

#[derive(Clone)]
struct NestedSpawnRegistration {
    context: Context,
    registered: Arc<AtomicBool>,
    outer_calls: Arc<AtomicUsize>,
    nested_calls: Arc<AtomicUsize>,
}

impl ThreadAware for NestedSpawnRegistration {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.context.relocate(source, destination);
    }
}

#[test]
fn spawn_hook_can_register_another_spawn_hook() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let context = runtime.scheduler().block_on(async |cx| cx).unwrap();
    let outer_calls = Arc::new(AtomicUsize::new(0));
    let nested_calls = Arc::new(AtomicUsize::new(0));

    context.control().on(HookFn::<TaskSpawn, _>::new(
        NestedSpawnRegistration {
            context: context.clone(),
            registered: Arc::new(AtomicBool::new(false)),
            outer_calls: Arc::clone(&outer_calls),
            nested_calls: Arc::clone(&nested_calls),
        },
        |_event, data| {
            data.outer_calls.fetch_add(1, Ordering::SeqCst);
            if !data.registered.swap(true, Ordering::SeqCst) {
                data.context.control().on(HookFn::<TaskSpawn, _>::new(
                    PanicCount(Arc::clone(&data.nested_calls)),
                    |_event, calls| {
                        calls.0.fetch_add(1, Ordering::SeqCst);
                    },
                ));
            }
        },
    ));

    context.scheduler().spawn(async |_| {}).join().unwrap();
    context.scheduler().spawn(async |_| {}).join().unwrap();

    assert_eq!(outer_calls.load(Ordering::SeqCst), 2);
    assert_eq!(nested_calls.load(Ordering::SeqCst), 1);
    runtime.stop().unwrap();
}

#[derive(Clone)]
struct RegistrationLog {
    log: SharedLog,
    installed: mpsc::Sender<()>,
}

impl ThreadAware for RegistrationLog {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
        assert!(
            self.installed.send(()).is_ok(),
            "the test retains the installation receiver until the hook is relocated"
        );
    }
}

#[derive(Clone)]
struct PanicCount(Arc<AtomicUsize>);

impl ThreadAware for PanicCount {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[derive(Clone)]
struct WorkerLocalData {
    calls: Arc<AtomicUsize>,
    local_calls: Cell<usize>,
}

static_assertions::assert_not_impl_any!(WorkerLocalData: Sync);

impl ThreadAware for WorkerLocalData {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[derive(Clone)]
struct InitSignal {
    calls: PanicCount,
    initialized: mpsc::Sender<()>,
}

impl ThreadAware for InitSignal {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

struct CloneCount(Arc<AtomicUsize>);

impl Clone for CloneCount {
    fn clone(&self) -> Self {
        self.0.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(&self.0))
    }
}

impl ThreadAware for CloneCount {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

struct LocalRegistrationProbe {
    clones: Arc<AtomicUsize>,
    relocated: mpsc::Sender<(ThreadId, ThreadId)>,
}

impl Clone for LocalRegistrationProbe {
    fn clone(&self) -> Self {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Self {
            clones: Arc::clone(&self.clones),
            relocated: self.relocated.clone(),
        }
    }
}

impl ThreadAware for LocalRegistrationProbe {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        let source = source
            .expect("runtime control supplies the source worker for worker-local registration")
            .id();
        assert!(
            self.relocated.send((source, destination.id())).is_ok(),
            "the test retains the relocation receiver until registration completes"
        );
    }
}

struct ShutdownDuringRegistration {
    control: RuntimeControl,
    original_dropped: Option<Arc<AtomicBool>>,
}

impl Clone for ShutdownDuringRegistration {
    fn clone(&self) -> Self {
        Self {
            control: self.control.clone(),
            original_dropped: None,
        }
    }
}

impl Drop for ShutdownDuringRegistration {
    fn drop(&mut self) {
        if let Some(original_dropped) = &self.original_dropped {
            original_dropped.store(true, Ordering::SeqCst);
        }
    }
}

impl ThreadAware for ShutdownDuringRegistration {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[derive(Clone)]
struct PanicProbe {
    count: Arc<AtomicUsize>,
    destination: Option<ThreadId>,
}

impl ThreadAware for PanicProbe {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.destination = Some(destination.id());
    }
}

#[derive(Clone)]
struct RemotePanicProbe {
    source: Option<ThreadId>,
    destination: Option<ThreadId>,
    installed: mpsc::Sender<(ThreadId, ThreadId)>,
    observed: mpsc::Sender<(ThreadId, ThreadId, ThreadId, u8)>,
}

impl ThreadAware for RemotePanicProbe {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        let source = source
            .expect("runtime control supplies the source worker for worker-local registration")
            .id();
        let destination = destination.id();
        self.source = Some(source);
        self.destination = Some(destination);
        assert!(
            self.installed.send((source, destination)).is_ok(),
            "the test retains the installation receiver until both workers install the hook"
        );
    }
}

#[derive(Clone)]
struct EventAddresses(Arc<Mutex<Vec<usize>>>);

impl ThreadAware for EventAddresses {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[expect(clippy::panic, reason = "this test helper creates a contained task panic")]
async fn panic_with_payload(payload: u8) {
    std::panic::panic_any(payload);
}

#[test]
fn panic_hook_covers_async_user_panics_only() {
    let count = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<TaskPanic, _>::new(
            PanicProbe {
                count: Arc::clone(&count),
                destination: None,
            },
            |event, data| {
                assert_eq!(data.destination, Some(std::thread::current().id()));
                assert_eq!(event.payload().downcast_ref::<&str>(), Some(&"async panic"));
                data.count.fetch_add(1, Ordering::SeqCst);
                event.set_ignore();
            },
        ))
        .build()
        .unwrap();

    assert!(
        runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| async { panic!("async panic") })
            .join()
            .unwrap_err()
            .is_panic()
    );
    assert!(
        runtime
            .scheduler()
            .spawn_blocking(|| panic!("blocking panic"))
            .join()
            .unwrap_err()
            .is_panic()
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 }).join().unwrap(), 42);
    runtime.stop().unwrap();
}

#[test]
fn panic_hook_selects_shutdown_from_the_payload() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<TaskPanic, _>::new((), |event, ()| {
            if event.payload().downcast_ref::<u32>() != Some(&2) {
                event.set_ignore();
            }
        }))
        .build()
        .unwrap();

    assert!(
        runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| async { std::panic::panic_any(1_u32) })
            .join()
            .unwrap_err()
            .is_panic()
    );
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 }).join().unwrap(), 42);

    assert!(
        runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| async { std::panic::panic_any(2_u32) })
            .join()
            .unwrap_err()
            .is_panic()
    );
    assert!(runtime.stop().is_err());
}

#[test]
fn control_appends_panic_hooks_in_registration_order() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = support::runtime_control(&runtime);

    let order = SharedLog(Arc::new(Mutex::new(Vec::new())));
    let (installed, installation) = mpsc::channel();
    control.on(HookFn::<TaskPanic, _>::new(
        RegistrationLog {
            log: order.clone(),
            installed,
        },
        |event, data| {
            data.log.push("ignore");
            event.set_ignore();
        },
    ));
    installation.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| async { panic!("ignored panic") })
            .join()
            .unwrap_err()
            .is_panic()
    );
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 }).join().unwrap(), 42);

    assert_eq!(order.take(), ["ignore"]);

    let (installed, installation) = mpsc::channel();
    control.on(HookFn::<TaskPanic, _>::new(
        RegistrationLog {
            log: order.clone(),
            installed,
        },
        |event, data| {
            data.log.push("shutdown");
            event.set_shutdown();
        },
    ));
    installation.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| async { panic!("shutdown panic") })
            .join()
            .unwrap_err()
            .is_panic()
    );
    assert_eq!(order.take(), ["ignore", "shutdown"]);
    assert!(runtime.stop().is_err());
}

#[test]
fn control_registration_applies_before_local_spawn() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));

    runtime
        .scheduler()
        .block_on(async |cx| {
            cx.control().on(TaskPanic::ignore());
            cx.control()
                .on(HookFn::<TaskPanic, _>::new(PanicCount(Arc::clone(&calls)), |_event, data| {
                    data.0.fetch_add(1, Ordering::SeqCst);
                }));
            let panic = cx.scheduler().spawn(async |_| panic!("local panic")).await.unwrap_err();
            assert!(panic.is_panic());
            assert_eq!(cx.scheduler().spawn(async |_| 42).await.unwrap(), 42);
        })
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    runtime.stop().unwrap();
}

#[test]
fn local_registration_clones_once_and_relocates_from_the_current_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let clones = Arc::new(AtomicUsize::new(0));
    let (relocated, relocation) = mpsc::channel();

    let source = runtime
        .scheduler()
        .block_on(async |cx| {
            let source = cx.thread().id();
            cx.control().on(HookFn::<TaskPanic, _>::new(
                LocalRegistrationProbe {
                    clones: Arc::clone(&clones),
                    relocated,
                },
                |_event, _data| {},
            ));
            assert_eq!(clones.load(Ordering::SeqCst), 1);
            source
        })
        .unwrap();

    assert_eq!(relocation.try_recv().unwrap(), (source, source));
    runtime.stop().unwrap();
}

#[test]
fn local_worker_spawn_retains_original_before_callback_starts_shutdown() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let original_dropped = Arc::new(AtomicBool::new(false));

    let result = runtime.scheduler().block_on(async |cx| {
        cx.control().on(HookFn::<WorkerSpawn, _>::new(
            ShutdownDuringRegistration {
                control: cx.control().clone(),
                original_dropped: Some(Arc::clone(&original_dropped)),
            },
            |_event, data| data.control.shutdown(),
        ));
        assert!(!original_dropped.load(Ordering::SeqCst));
    });

    result.unwrap();
    runtime.stop().unwrap();
    assert!(original_dropped.load(Ordering::SeqCst));
}

#[test]
fn worker_registration_relocates_and_runs_on_every_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let (installed, installations) = mpsc::channel();
    let (observed, observations) = mpsc::channel();

    let (workers, source, scheduler) = runtime
        .scheduler()
        .block_on(async |cx| {
            let workers: Vec<_> = cx.control().workers().into_iter().cloned().collect();
            let source = cx.thread().id();
            cx.control().on(HookFn::<TaskPanic, _>::new(
                RemotePanicProbe {
                    source: None,
                    destination: None,
                    installed,
                    observed,
                },
                |event, data| {
                    let payload = *event.payload().downcast_ref::<u8>().unwrap();
                    assert!(
                        data.observed
                            .send((
                                std::thread::current().id(),
                                data.source.unwrap(),
                                data.destination.unwrap(),
                                payload,
                            ))
                            .is_ok(),
                        "the test retains the observation receiver until both callbacks run"
                    );
                    event.set_ignore();
                },
            ));
            (workers, source, cx.scheduler().clone())
        })
        .unwrap();

    let installed: Vec<_> = (0..workers.len())
        .map(|_| installations.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    assert!(workers.iter().all(|worker| installed.contains(&(source, worker.id()))));

    for (payload, worker) in (0_u8..).zip(&workers) {
        assert!(
            scheduler
                .spawn_at(worker, payload, panic_with_payload)
                .join()
                .unwrap_err()
                .is_panic()
        );
    }

    for _ in &workers {
        let (current, observed_source, destination, payload) = observations.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(observed_source, source);
        assert_eq!(current, destination);
        assert_eq!(destination, workers[usize::from(payload)].id());
    }
    runtime.stop().unwrap();
}

#[test]
fn panic_callbacks_share_one_borrowed_event() {
    let addresses = EventAddresses(Arc::new(Mutex::new(Vec::new())));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<TaskPanic, _>::new(addresses.clone(), |event, data| {
            let address = std::ptr::from_mut(event).addr();
            data.0.lock().unwrap().push(address);
            event.set_ignore();
        }))
        .on(HookFn::<TaskPanic, _>::new(addresses.clone(), |event, data| {
            let address = std::ptr::from_mut(event).addr();
            data.0.lock().unwrap().push(address);
            event.set_ignore();
        }))
        .build()
        .unwrap();

    assert!(
        runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| async { panic!("shared event") })
            .join()
            .unwrap_err()
            .is_panic()
    );
    let addresses = addresses.0.lock().unwrap();
    assert_eq!(addresses.len(), 2);
    assert_eq!(addresses[0], addresses[1]);
    drop(addresses);
    runtime.stop().unwrap();
}

#[test]
fn control_worker_spawn_runs_once_on_every_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let control = runtime.scheduler().block_on(async |cx| cx.control().clone()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (initialized, initialization) = mpsc::channel();

    control.on(HookFn::<WorkerSpawn, _>::new(
        InitSignal {
            calls: PanicCount(Arc::clone(&calls)),
            initialized,
        },
        |_event, data| {
            data.calls.0.fetch_add(1, Ordering::SeqCst);
            data.initialized.send(()).unwrap();
        },
    ));
    for _ in control.workers() {
        initialization.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    runtime.stop().unwrap();
}

#[derive(Clone)]
struct RetainedContext {
    context: Context,
    _lifetime: Arc<()>,
}

impl Drop for RetainedContext {
    fn drop(&mut self) {
        self.context.control().on(HookFn::<WorkerSpawn, _>::new((), |_event, ()| {}));
    }
}

impl ThreadAware for RetainedContext {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.context.relocate(source, destination);
    }
}

struct BlockingDrop {
    retained: bool,
    started: mpsc::Sender<()>,
    release: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl Clone for BlockingDrop {
    fn clone(&self) -> Self {
        Self {
            retained: false,
            started: self.started.clone(),
            release: Arc::clone(&self.release),
        }
    }
}

#[expect(clippy::unwrap_used, reason = "test fixture failures use unwrap for concise backtraces")]
impl Drop for BlockingDrop {
    fn drop(&mut self) {
        if self.retained {
            self.started.send(()).unwrap();
            self.release.lock().unwrap().recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }
}

impl ThreadAware for BlockingDrop {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[test]
fn retained_worker_spawn_data_is_released_during_shutdown() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let lifetime = Arc::new(());
    let weak = Arc::downgrade(&lifetime);

    runtime
        .scheduler()
        .block_on(async move |cx| {
            cx.control().on(HookFn::<WorkerSpawn, _>::new(
                RetainedContext {
                    context: cx.clone(),
                    _lifetime: lifetime,
                },
                |_event, _data| {},
            ));
        })
        .unwrap();

    assert!(weak.upgrade().is_some());
    runtime.stop().unwrap();
    assert!(weak.upgrade().is_none());
}

#[test]
fn shutdown_does_not_drop_retained_hook_data() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let (started, drop_started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let control = runtime
        .scheduler()
        .block_on(async move |cx| {
            cx.control().on(HookFn::<WorkerSpawn, _>::new(
                BlockingDrop {
                    retained: true,
                    started,
                    release: Arc::new(Mutex::new(released)),
                },
                |_event, _data| {},
            ));
            cx.control().clone()
        })
        .unwrap();

    control.shutdown();
    assert!(drop_started.try_recv().is_err());

    let (stopped, stop_result) = mpsc::sync_channel(1);
    let stop = std::thread::spawn(move || stopped.send(runtime.stop()).unwrap());
    drop_started.recv_timeout(Duration::from_secs(5)).unwrap();
    release.send(()).unwrap();
    stop_result.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    stop.join().unwrap();
}

#[test]
fn control_ignores_registration_after_shutdown_starts() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = support::runtime_control(&runtime);
    let clones = Arc::new(AtomicUsize::new(0));

    control.shutdown();
    control.on(HookFn::<TaskPanic, _>::new(CloneCount(Arc::clone(&clones)), |_event, _data| {}));

    assert_eq!(clones.load(Ordering::SeqCst), 0);
    runtime.stop().unwrap();
}

#[test]
fn control_clones_nonretained_hook_data_for_each_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = support::runtime_control(&runtime);
    let clones = Arc::new(AtomicUsize::new(0));

    control.on(HookFn::<TaskPanic, _>::new(CloneCount(Arc::clone(&clones)), |_event, _data| {}));

    assert_eq!(clones.load(Ordering::SeqCst), 1);
    runtime.stop().unwrap();
}
