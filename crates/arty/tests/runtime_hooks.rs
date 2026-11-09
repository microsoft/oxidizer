// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime hook contracts.

#![cfg(feature = "rt")]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use arty::core::{Thread, ThreadAware};
use arty::runtime::hook::{OnPanic, OnStart};
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
    control.on_with::<OnPanic, _>(PanicCount(Arc::clone(&calls)), |_, count| {
        count.0.fetch_add(1, Ordering::SeqCst);
    });

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    runtime.stop().unwrap();
}
