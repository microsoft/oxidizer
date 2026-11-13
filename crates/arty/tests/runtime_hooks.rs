// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime hook contracts.

#![cfg(feature = "rt")]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::ThreadId;
use std::time::Duration;

use arty::core::{Thread, ThreadAware};
use arty::runtime::hook::{HookFn, Panic, WorkerInit};
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
