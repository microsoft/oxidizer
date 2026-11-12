// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime hook contracts.

#![cfg(feature = "rt")]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use arty::core::{Thread, ThreadAware};
use arty::runtime::hook::{HookFn, Panic};
use arty::runtime::{Runtime, WorkersPolicy};
use support::JoinHandleExt as _;

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
struct PanicCount(Arc<AtomicUsize>);

impl ThreadAware for PanicCount {
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
struct EventAddresses(Arc<Mutex<Vec<usize>>>);

impl ThreadAware for EventAddresses {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
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
fn control_appends_panic_hooks_in_registration_order() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = support::runtime_control(&runtime);

    let order = SharedLog(Arc::new(Mutex::new(Vec::new())));
    control.on(HookFn::<Panic, _>::new(order.clone(), |event, data| {
        data.push("ignore");
        event.ignore();
    }));
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

    control.on(HookFn::<Panic, _>::new(order.clone(), |event, data| {
        data.push("shutdown");
        event.shutdown();
    }));
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
fn control_ignores_registration_after_shutdown_starts() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let control = support::runtime_control(&runtime);
    let calls = Arc::new(AtomicUsize::new(0));

    control.request_stop();
    control.on(HookFn::<Panic, _>::new(PanicCount(Arc::clone(&calls)), |_event, data| {
        data.0.fetch_add(1, Ordering::SeqCst);
    }));

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    runtime.stop().unwrap();
}
