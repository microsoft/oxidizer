// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Register startup and panic hooks with worker-local data.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::ThreadId;

use arty::core::{Thread, ThreadAware};
use arty::runtime::hook::{OnPanic, OnStart};
use arty::runtime::{Runtime, WorkersPolicy};

#[derive(Default)]
struct Counts {
    starts: AtomicUsize,
    panics: AtomicUsize,
}

#[derive(Clone)]
struct HookData {
    counts: Arc<Counts>,
    worker: Option<ThreadId>,
}

impl ThreadAware for HookData {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.worker = Some(destination.id());
    }
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let counts = Arc::new(Counts::default());
    let data = HookData {
        counts: Arc::clone(&counts),
        worker: None,
    };
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .on_with::<OnStart, _>(data.clone(), |event, data| {
            assert_eq!(data.worker, Some(event.thread().id()));
            data.counts.starts.fetch_add(1, Ordering::Relaxed);
        })
        .build()?;

    let (control, scheduler) = runtime
        .scheduler()
        .block_on(async |cx| (cx.control().clone(), cx.scheduler().clone()))?;
    control.on_with::<OnPanic, _>(data, |event, data| {
        assert_eq!(data.worker, Some(std::thread::current().id()));
        data.counts.panics.fetch_add(1, Ordering::Relaxed);
        event.ignore();
    });

    let panic = futures::executor::block_on(scheduler.spawn(async |_| panic!("contained panic"))).unwrap_err();
    assert!(panic.is_panic());
    assert_eq!(futures::executor::block_on(scheduler.spawn(async |_| 42))?, 42);

    assert_eq!(counts.starts.load(Ordering::Relaxed), 2);
    assert_eq!(counts.panics.load(Ordering::Relaxed), 1);
    println!(
        "starts={}, panics={}",
        counts.starts.load(Ordering::Relaxed),
        counts.panics.load(Ordering::Relaxed)
    );

    runtime.stop()?;
    Ok(())
}
