// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Register runtime hooks and add a scope around each async task poll.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use std::thread::ThreadId;

use arty::core::{Thread, ThreadAware};
use arty::runtime::hook::{OnPanic, OnSpawn, OnStart, TaskPoll};
use arty::runtime::{Runtime, WorkersPolicy};

#[derive(Default)]
struct Counts {
    starts: AtomicUsize,
    spawns: AtomicUsize,
    polls: AtomicUsize,
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

struct CountPolls {
    counts: Arc<Counts>,
    worker: Option<ThreadId>,
}

impl ThreadAware for CountPolls {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.worker = Some(destination.id());
    }
}

impl TaskPoll for CountPolls {
    fn poll(&mut self, task: &mut dyn FnMut() -> Poll<()>) -> Poll<()> {
        assert_eq!(self.worker, Some(std::thread::current().id()));
        self.counts.polls.fetch_add(1, Ordering::Relaxed);
        task()
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
        .on_with::<OnSpawn, _>(data, |event, data| {
            assert_eq!(data.worker, Some(event.thread().id()));
            data.counts.spawns.fetch_add(1, Ordering::Relaxed);
            event.register(CountPolls {
                counts: Arc::clone(&data.counts),
                worker: None,
            });
        })
        .build()?;

    let (control, scheduler) = runtime
        .scheduler()
        .block_on(async |cx| (cx.control().clone(), cx.scheduler().clone()))?;

    // A contained async or blocking panic now leaves the runtime available.
    control.on::<OnPanic>(|event| event.ignore())?;

    let answer = futures::executor::block_on(scheduler.spawn(async |_| 42))?;
    assert_eq!(answer, 42);

    assert_eq!(counts.starts.load(Ordering::Relaxed), 2);
    assert!(counts.spawns.load(Ordering::Relaxed) >= 2);
    assert!(counts.polls.load(Ordering::Relaxed) >= 2);
    println!(
        "starts={}, spawns={}, polls={}",
        counts.starts.load(Ordering::Relaxed),
        counts.spawns.load(Ordering::Relaxed),
        counts.polls.load(Ordering::Relaxed)
    );

    runtime.stop()?;
    Ok(())
}
