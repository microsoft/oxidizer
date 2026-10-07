// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Safe input relocation panics fail the task before invoking its factory.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use panic_support::runtime;
use thread_aware::{Thread, ThreadAware};

testing_aids::init_tracing!();

#[derive(Clone)]
struct PanicRelocation {
    relocations: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    factories: Arc<AtomicUsize>,
}

impl ThreadAware for PanicRelocation {
    fn relocate(&mut self, _: Option<&Thread>, destination: &Thread) {
        assert_eq!(destination.id(), std::thread::current().id());
        self.relocations.fetch_add(1, Ordering::SeqCst);
        panic!("user payload relocation");
    }
}

impl Drop for PanicRelocation {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

fn input() -> PanicRelocation {
    PanicRelocation {
        relocations: Arc::new(AtomicUsize::new(0)),
        drops: Arc::new(AtomicUsize::new(0)),
        factories: Arc::new(AtomicUsize::new(0)),
    }
}

#[test]
fn runtime_input_relocation_panic_is_a_join_error() {
    let runtime = runtime();
    let data = input();
    let relocations = Arc::clone(&data.relocations);
    let drops = Arc::clone(&data.drops);
    let factories = Arc::clone(&data.factories);
    let task = runtime.scheduler().spawn_anywhere(data, |_, data| {
        data.factories.fetch_add(1, Ordering::SeqCst);
        std::future::ready(())
    });
    assert!(task.wait().unwrap_err().is_panic());
    assert_eq!(relocations.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 7 }).wait().unwrap(), 7);
    runtime.stop().unwrap();
}

#[test]
fn worker_input_relocation_panics_are_join_errors() {
    let runtime = runtime();
    let data = input();
    let relocations = Arc::clone(&data.relocations);
    let drops = Arc::clone(&data.drops);
    let factories = Arc::clone(&data.factories);
    runtime
        .scheduler()
        .block_on(async move |cx| {
            let task = cx.scheduler().spawn_anywhere(data.clone(), |data| {
                data.factories.fetch_add(1, Ordering::SeqCst);
                std::future::ready(())
            });
            assert!(task.await.unwrap_err().is_panic());
            let tasks = cx.scheduler().spawn_everywhere(data, |data| {
                data.factories.fetch_add(1, Ordering::SeqCst);
                std::future::ready(())
            });
            for task in tasks {
                assert!(task.await.unwrap_err().is_panic());
            }
            assert_eq!(cx.scheduler().spawn(async |_| 7).await.unwrap(), 7);
        })
        .unwrap();
    assert_eq!(relocations.load(Ordering::SeqCst), 2);
    assert_eq!(drops.load(Ordering::SeqCst), 3);
    assert_eq!(factories.load(Ordering::SeqCst), 0);
    runtime.stop().unwrap();
}
