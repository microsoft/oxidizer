// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime hook contracts.

#![cfg(feature = "rt")]

mod support;

use std::cell::Cell;
use std::collections::HashSet;
use std::future::poll_fn;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::thread::{self, ThreadId};

use arty::core::{Thread, ThreadAware};
use arty::runtime::hook::{OnPanic, OnSpawn, OnStart, TaskPoll};
use arty::runtime::{Runtime, WorkersPolicy};
use support::JoinHandleExt as _;

#[derive(Clone)]
struct StartProbe {
    destination: Option<ThreadId>,
    observations: Arc<Mutex<Vec<(ThreadId, ThreadId)>>>,
}

impl ThreadAware for StartProbe {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.destination = Some(destination.id());
    }
}

#[test]
fn builder_start_hook_runs_after_relocation_and_before_build_returns() {
    let observations = Arc::new(Mutex::new(Vec::new()));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .on_with::<OnStart, _>(
            StartProbe {
                destination: None,
                observations: Arc::clone(&observations),
            },
            |event, probe| {
                probe
                    .observations
                    .lock()
                    .unwrap()
                    .push((event.thread().id(), probe.destination.unwrap()));
            },
        )
        .build()
        .unwrap();

    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 2);
    assert!(observations.iter().all(|(event, relocated)| event == relocated));
    assert_ne!(observations[0].0, observations[1].0);
    drop(observations);
    runtime.stop().unwrap();
}

#[derive(Clone)]
struct SharedLog(Arc<Mutex<Vec<&'static str>>>);

impl SharedLog {
    fn push(&self, value: &'static str) {
        self.0.lock().unwrap().push(value);
    }

    fn take(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.0.lock().unwrap())
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
    fn poll(&mut self, task: &mut dyn FnMut() -> Poll<()>) -> Poll<()> {
        assert_eq!(self.destination, Some(thread::current().id()));
        self.log.push(self.before);
        let result = task();
        self.log.push(self.after);
        result
    }
}

#[test]
fn spawn_hook_wraps_the_complete_task_poll_in_registration_order() {
    let log = SharedLog(Arc::new(Mutex::new(Vec::new())));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on_with::<OnSpawn, _>(log.clone(), |event, log| {
            log.push("outer spawn");
            event.register(PollRecorder {
                before: "outer before",
                after: "outer after",
                destination: None,
                log: log.clone(),
            });
        })
        .on_with::<OnSpawn, _>(log.clone(), |event, log| {
            log.push("inner spawn");
            event.register(PollRecorder {
                before: "inner before",
                after: "inner after",
                destination: None,
                log: log.clone(),
            });
        })
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
        .on::<OnPanic>(|event| event.ignore())
        .on_with::<OnSpawn, _>(log.clone(), |event, log| {
            event.register(PollRecorder {
                before: "before",
                after: "after",
                destination: None,
                log: log.clone(),
            });
        })
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
    fn poll(&mut self, _task: &mut dyn FnMut() -> Poll<()>) -> Poll<()> {
        panic!("poll hook panic");
    }
}

#[test]
fn poll_hook_panic_fails_the_worker_instead_of_the_task() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on::<OnSpawn>(|event| event.register(PanickingPoll))
        .build()
        .unwrap();

    let error = runtime.scheduler().spawn_anywhere((), |_, ()| async {}).join().unwrap_err();

    assert!(error.is_shutdown());
    assert!(!error.is_panic());
    assert!(runtime.stop().is_err());
}

struct SkippingPoll;

impl ThreadAware for SkippingPoll {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl TaskPoll for SkippingPoll {
    fn poll(&mut self, _task: &mut dyn FnMut() -> Poll<()>) -> Poll<()> {
        Poll::Pending
    }
}

#[test]
fn poll_hook_must_call_the_task() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on::<OnSpawn>(|event| event.register(SkippingPoll))
        .build()
        .unwrap();

    let error = runtime.scheduler().spawn_anywhere((), |_, ()| async {}).join().unwrap_err();

    assert!(error.is_shutdown());
    assert!(runtime.stop().is_err());
}

struct DoublePoll;

impl ThreadAware for DoublePoll {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

impl TaskPoll for DoublePoll {
    fn poll(&mut self, task: &mut dyn FnMut() -> Poll<()>) -> Poll<()> {
        let result = task();
        _ = task();
        result
    }
}

#[test]
fn poll_hook_must_not_call_the_task_twice() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on::<OnSpawn>(|event| event.register(DoublePoll))
        .build()
        .unwrap();

    let error = runtime.scheduler().spawn_anywhere((), |_, ()| async {}).join().unwrap_err();

    assert!(error.is_shutdown());
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
    fn poll(&mut self, task: &mut dyn FnMut() -> Poll<()>) -> Poll<()> {
        struct Reset;

        impl Drop for Reset {
            fn drop(&mut self) {
                POLL_SCOPE_DEPTH.set(0);
            }
        }

        POLL_SCOPE_DEPTH.with(|depth| {
            assert_eq!(depth.replace(1), 0);
        });
        let reset = Reset;
        let result = task();
        drop(reset);
        result
    }
}

#[test]
fn poll_hook_scope_is_removed_before_the_next_poll() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on::<OnSpawn>(|event| event.register(ScopedPoll))
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
        .on_with::<OnSpawn, _>(PanicCount(Arc::clone(&calls)), |_, count| {
            count.0.fetch_add(1, Ordering::SeqCst);
        })
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

    control.on_with::<OnSpawn, _>(
        LateSpawnProbe {
            calls: Arc::clone(&calls),
            destinations: Arc::clone(&destinations),
        },
        |event, probe| {
            assert_eq!(event.thread().id(), thread::current().id());
            probe.calls.fetch_add(1, Ordering::SeqCst);
        },
    );

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

#[derive(Clone)]
struct PanicCount(Arc<AtomicUsize>);

impl ThreadAware for PanicCount {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

#[test]
fn panic_hook_covers_async_and_blocking_user_panics() {
    let count = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on_with::<OnPanic, _>(PanicCount(Arc::clone(&count)), |event, count| {
            count.0.fetch_add(1, Ordering::SeqCst);
            event.ignore();
        })
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
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 }).join().unwrap(), 42);
    runtime.stop().unwrap();
}

#[test]
fn control_appends_panic_hooks_in_registration_order() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = support::runtime_control(&runtime);

    let order = SharedLog(Arc::new(Mutex::new(Vec::new())));
    control.on_with::<OnPanic, _>(order.clone(), |event, order| {
        order.push("ignore");
        event.ignore();
    });
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

    control.on_with::<OnPanic, _>(order.clone(), |event, order| {
        order.push("shutdown");
        event.shutdown();
    });
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
fn late_start_registration_does_not_replay_worker_start() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let control = support::runtime_control(&runtime);
    let calls = Arc::new(AtomicUsize::new(0));

    control.on_with::<OnStart, _>(PanicCount(Arc::clone(&calls)), |_, count| {
        count.0.fetch_add(1, Ordering::SeqCst);
    });

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    runtime.stop().unwrap();
}

#[test]
fn control_ignores_registration_after_shutdown_starts() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = support::runtime_control(&runtime);
    let calls = Arc::new(AtomicUsize::new(0));

    control.request_stop();
    control.on_with::<OnSpawn, _>(PanicCount(Arc::clone(&calls)), |_, count| {
        count.0.fetch_add(1, Ordering::SeqCst);
    });

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    runtime.stop().unwrap();
}
