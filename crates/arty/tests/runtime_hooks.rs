// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime hook contracts.

#![cfg(feature = "rt")]

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
use arty::runtime::hook::{HookFn, Panic, Spawn, TaskPoll, WorkerInit};
use arty::runtime::{Runtime, WorkersPolicy};
use arty::task::Context;
use support::JoinHandleExt as _;

#[derive(Clone)]
struct WorkerInitProbe {
    destination: Option<ThreadId>,
    observed: mpsc::Sender<WorkerInitObservation>,
}

impl ThreadAware for WorkerInitProbe {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.destination = Some(destination.id());
    }
}

struct WorkerInitObservation {
    callback_thread: ThreadId,
    event_thread: ThreadId,
    relocated_thread: ThreadId,
}

#[test]
fn worker_init_runs_once_per_worker() {
    let (observed, observations) = mpsc::channel();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .on(HookFn::<WorkerInit, _>::new(
            WorkerInitProbe {
                destination: None,
                observed,
            },
            |event, data| {
                data.observed
                    .send(WorkerInitObservation {
                        callback_thread: std::thread::current().id(),
                        event_thread: event.thread().id(),
                        relocated_thread: data.destination.unwrap(),
                    })
                    .unwrap();
            },
        ))
        .build()
        .unwrap();

    let observations = (0..2).map(|_| observations.try_recv().unwrap()).collect::<Vec<_>>();
    assert_eq!(observations.len(), 2);
    assert_ne!(observations[0].event_thread, observations[1].event_thread);
    for observation in observations {
        assert_eq!(observation.callback_thread, observation.event_thread);
        assert_eq!(observation.relocated_thread, observation.event_thread);
    }
    runtime.stop().unwrap();
}

#[test]
fn builder_retains_worker_init_data_until_shutdown() {
    let calls = Arc::new(AtomicUsize::new(0));
    let lifetime = Arc::downgrade(&calls);
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .on(HookFn::<WorkerInit, _>::new(PanicCount(calls), |_event, data| {
            data.0.fetch_add(1, Ordering::SeqCst);
        }))
        .build()
        .unwrap();

    assert_eq!(lifetime.upgrade().unwrap().load(Ordering::SeqCst), 2);
    runtime.stop().unwrap();
    assert!(lifetime.upgrade().is_none());
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

struct PollRecorder {
    before: &'static str,
    after: &'static str,
    destination: Option<ThreadId>,
    log: SharedLog,
}

impl ThreadAware for PollRecorder {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.destination = Some(destination.id());
    }
}

impl TaskPoll for PollRecorder {
    fn begin(&mut self) {
        assert_eq!(self.destination, Some(thread::current().id()));
        self.log.push(self.before);
    }

    fn end(&mut self) {
        self.log.push(self.after);
    }
}

#[test]
fn spawn_hook_brackets_the_complete_task_poll_in_registration_order() {
    let log = SharedLog(Arc::new(Mutex::new(Vec::new())));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<Spawn, _>::new(log.clone(), |event, log| {
            log.push("outer spawn");
            event.register(PollRecorder {
                before: "outer before",
                after: "outer after",
                destination: None,
                log: log.clone(),
            });
        }))
        .on(HookFn::<Spawn, _>::new(log.clone(), |event, log| {
            log.push("inner spawn");
            event.register(PollRecorder {
                before: "inner before",
                after: "inner after",
                destination: None,
                log: log.clone(),
            });
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
        .on(Panic::IGNORE)
        .on(HookFn::<Spawn, _>::new(log.clone(), |event, log| {
            event.register(PollRecorder {
                before: "before",
                after: "after",
                destination: None,
                log: log.clone(),
            });
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

impl TaskPoll for PanickingPoll {
    fn begin(&mut self) {
        panic!("poll hook panic");
    }

    fn end(&mut self) {}
}

#[test]
fn poll_hook_panic_fails_the_worker_instead_of_the_task() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<Spawn, _>::new((), |event, ()| event.register(PanickingPoll)))
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

impl TaskPoll for ScopedPoll {
    fn begin(&mut self) {
        POLL_SCOPE_DEPTH.with(|depth| {
            assert_eq!(depth.replace(1), 0);
        });
    }

    fn end(&mut self) {
        POLL_SCOPE_DEPTH.set(0);
    }
}

#[test]
fn poll_hook_scope_is_removed_before_the_next_poll() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<Spawn, _>::new((), |event, ()| event.register(ScopedPoll)))
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
        .on(HookFn::<Spawn, _>::new(PanicCount(Arc::clone(&calls)), |event, count| {
            assert!(event.thread().is_none());
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
fn spawn_hook_runs_for_a_detached_scheduler() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(HookFn::<Spawn, _>::new(PanicCount(Arc::clone(&calls)), |_, count| {
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
    relocation: Arc<Mutex<Option<(Option<ThreadId>, ThreadId)>>>,
    began: Arc<AtomicBool>,
}

impl ThreadAware for NeutralSpawnProbe {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

struct RelocatedPoll {
    factory_created: Arc<AtomicBool>,
    relocation: Arc<Mutex<Option<(Option<ThreadId>, ThreadId)>>>,
    began: Arc<AtomicBool>,
}

impl ThreadAware for RelocatedPoll {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        assert!(self.factory_created.load(Ordering::Acquire));
        self.relocation.lock().unwrap().replace((source.map(Thread::id), destination.id()));
    }
}

impl TaskPoll for RelocatedPoll {
    fn begin(&mut self) {
        assert!(self.factory_created.load(Ordering::Acquire));
        let destination = self.relocation.lock().unwrap().unwrap().1;
        assert_eq!(destination, thread::current().id());
        self.began.store(true, Ordering::Release);
    }

    fn end(&mut self) {}
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
fn detached_scheduler_runs_neutral_spawn_then_relocates_poll_after_factory() {
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
            cx.control().on(HookFn::<Spawn, _>::new(
                NeutralSpawnProbe {
                    factory_created: Arc::clone(&factory_created),
                    relocation: Arc::clone(&relocation),
                    began: Arc::clone(&began),
                },
                |event, probe| {
                    assert!(event.thread().is_none());
                    probe.factory_created.store(false, Ordering::Release);
                    event.register(RelocatedPoll {
                        factory_created: Arc::clone(&probe.factory_created),
                        relocation: Arc::clone(&probe.relocation),
                        began: Arc::clone(&probe.began),
                    });
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

    control.on(HookFn::<Spawn, _>::new(
        LateSpawnProbe {
            calls: Arc::clone(&calls),
            destinations: Arc::clone(&destinations),
        },
        |event, probe| {
            assert!(event.thread().is_none());
            probe.calls.fetch_add(1, Ordering::SeqCst);
        },
    ));

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

    control.request_stop();
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
            cx.control().on(HookFn::<Spawn, _>::new(PanicCount(Arc::clone(&calls)), |_, count| {
                count.0.fetch_add(1, Ordering::SeqCst);
            }));
            cx.scheduler().spawn(async |_| {}).await.unwrap();
        })
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
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
        .on(HookFn::<Panic, _>::new(
            PanicProbe {
                count: Arc::clone(&count),
                destination: None,
            },
            |event, data| {
                assert_eq!(data.destination, Some(std::thread::current().id()));
                assert_eq!(event.payload().downcast_ref::<&str>(), Some(&"async panic"));
                data.count.fetch_add(1, Ordering::SeqCst);
                event.ignore();
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
        .on(HookFn::<Panic, _>::new((), |event, ()| {
            if event.payload().downcast_ref::<u32>() != Some(&2) {
                event.ignore();
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
    control.on(HookFn::<Panic, _>::new(
        RegistrationLog {
            log: order.clone(),
            installed,
        },
        |event, data| {
            data.log.push("ignore");
            event.ignore();
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
    control.on(HookFn::<Panic, _>::new(
        RegistrationLog {
            log: order.clone(),
            installed,
        },
        |event, data| {
            data.log.push("shutdown");
            event.shutdown();
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
            cx.control().on(Panic::IGNORE);
            cx.control()
                .on(HookFn::<Panic, _>::new(PanicCount(Arc::clone(&calls)), |_event, data| {
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
fn worker_registration_relocates_and_runs_on_every_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let (installed, installations) = mpsc::channel();
    let (observed, observations) = mpsc::channel();

    let (workers, source, scheduler) = runtime
        .scheduler()
        .block_on(async |cx| {
            let workers: Vec<_> = cx.control().workers().into_iter().cloned().collect();
            let source = cx.thread().id();
            cx.control().on(HookFn::<Panic, _>::new(
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
                    event.ignore();
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
        .on(HookFn::<Panic, _>::new(addresses.clone(), |event, data| {
            let address = std::ptr::from_mut(event).addr();
            data.0.lock().unwrap().push(address);
            event.ignore();
        }))
        .on(HookFn::<Panic, _>::new(addresses.clone(), |event, data| {
            let address = std::ptr::from_mut(event).addr();
            data.0.lock().unwrap().push(address);
            event.ignore();
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
fn control_worker_init_runs_once_on_every_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let (control, workers, scheduler) = runtime
        .scheduler()
        .block_on(async |cx| {
            (
                cx.control().clone(),
                cx.control().workers().into_iter().cloned().collect::<Vec<_>>(),
                cx.scheduler().clone(),
            )
        })
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));

    control.on(HookFn::<WorkerInit, _>::new(PanicCount(Arc::clone(&calls)), |_event, data| {
        data.0.fetch_add(1, Ordering::SeqCst);
    }));
    for worker in workers {
        scheduler.spawn_at(&worker, (), |()| async {}).join().unwrap();
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
        self.context.control().on(HookFn::<WorkerInit, _>::new((), |_event, ()| {}));
    }
}

impl ThreadAware for RetainedContext {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.context.relocate(source, destination);
    }
}

#[test]
fn retained_worker_init_data_is_released_during_shutdown() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let lifetime = Arc::new(());
    let weak = Arc::downgrade(&lifetime);

    runtime
        .scheduler()
        .block_on(async move |cx| {
            cx.control().on(HookFn::<WorkerInit, _>::new(
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
fn control_ignores_registration_after_shutdown_starts() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = support::runtime_control(&runtime);
    let clones = Arc::new(AtomicUsize::new(0));

    control.request_stop();
    control.on(HookFn::<Panic, _>::new(CloneCount(Arc::clone(&clones)), |_event, _data| {}));

    assert_eq!(clones.load(Ordering::SeqCst), 0);
    runtime.stop().unwrap();
}
