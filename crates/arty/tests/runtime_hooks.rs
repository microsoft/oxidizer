// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime hook contracts.

#![cfg(feature = "rt")]

mod support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::ThreadId;
use std::time::Duration;

use arty::core::{Thread, ThreadAware};
use arty::runtime::hook::{HookFn, Panic, WorkerInit};
use arty::runtime::policy::WorkersPolicy;
use arty::runtime::{Runtime, RuntimeControl};
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
    callback: ThreadId,
    event: ThreadId,
    relocated: ThreadId,
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
        .on(HookFn::<Panic, _>::new(
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
        .on(HookFn::<Panic, _>::new((), |event, ()| {
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
    control.on(HookFn::<Panic, _>::new(
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
    control.on(HookFn::<Panic, _>::new(
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
            cx.control().on(Panic::ignore());
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
fn local_registration_clones_once_and_relocates_from_the_current_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let clones = Arc::new(AtomicUsize::new(0));
    let (relocated, relocation) = mpsc::channel();

    let source = runtime
        .scheduler()
        .block_on(async |cx| {
            let source = cx.thread().id();
            cx.control().on(HookFn::<Panic, _>::new(
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
fn local_worker_init_retains_original_before_callback_starts_shutdown() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let original_dropped = Arc::new(AtomicBool::new(false));

    let result = runtime.scheduler().block_on(async |cx| {
        cx.control().on(HookFn::<WorkerInit, _>::new(
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
            let workers: Vec<_> = cx.control().workers().iter().collect();
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
        .on(HookFn::<Panic, _>::new(addresses.clone(), |event, data| {
            let address = std::ptr::from_mut(event).addr();
            data.0.lock().unwrap().push(address);
            event.set_ignore();
        }))
        .on(HookFn::<Panic, _>::new(addresses.clone(), |event, data| {
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
fn control_worker_init_runs_once_on_every_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let control = runtime.scheduler().block_on(async |cx| cx.control().clone()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (initialized, initialization) = mpsc::channel();

    control.on(HookFn::<WorkerInit, _>::new(
        InitSignal {
            calls: PanicCount(Arc::clone(&calls)),
            initialized,
        },
        |_event, data| {
            data.calls.0.fetch_add(1, Ordering::SeqCst);
            data.initialized.send(()).unwrap();
        },
    ));
    for _ in control.workers().iter() {
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
        self.context.control().on(HookFn::<WorkerInit, _>::new((), |_event, ()| {}));
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
fn control_shutdown_does_not_drop_retained_hook_data() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let (started, drop_started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let control = runtime
        .scheduler()
        .block_on(async move |cx| {
            cx.control().on(HookFn::<WorkerInit, _>::new(
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
    control.on(HookFn::<Panic, _>::new(CloneCount(Arc::clone(&clones)), |_event, _data| {}));

    assert_eq!(clones.load(Ordering::SeqCst), 0);
    runtime.stop().unwrap();
}

#[test]
fn control_clones_nonretained_hook_data_for_each_worker() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = support::runtime_control(&runtime);
    let clones = Arc::new(AtomicUsize::new(0));

    control.on(HookFn::<Panic, _>::new(CloneCount(Arc::clone(&clones)), |_event, _data| {}));

    assert_eq!(clones.load(Ordering::SeqCst), 1);
    runtime.stop().unwrap();
}
