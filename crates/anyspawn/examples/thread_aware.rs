// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Per-core task spawning with a custom [`SpawnCustom`] implementation.
//!
//! Demonstrates creating a spawner whose [`Scheduler`] owns the threads in its
//! runtime. Relocation selects the destination from that thread list, so each
//! scheduler clone knows which thread it represents.
//!
//! In production the data might hold a core-local work queue, metrics
//! counter, or connection pool instead of a simple index.

use anyspawn::{BoxedBlockingTask, BoxedFuture, SpawnCustom, Spawner};
use thread_aware::closure::ThreadAwareAsyncFnOnce;
use thread_aware::thread::ThreadBuilder;
use thread_aware::{Thread, ThreadAware};

#[tokio::main]
async fn main() {
    // Simulate a runtime with one thread on each of two NUMA nodes.
    let builder = ThreadBuilder::default();
    let thread0 = builder.build(std::thread::current().id());
    let thread1_builder = builder.with_numa_node(1);
    let thread1 = std::thread::spawn(move || thread1_builder.build(std::thread::current().id()))
        .join()
        .expect("coordinate thread must finish");

    let scheduler = Scheduler::new([thread0.clone(), thread1.clone()]);
    for thread in scheduler.threads() {
        println!("runtime thread: {thread:?}");
    }

    let spawner = Spawner::new_custom("per-thread", scheduler);
    let _on_thread0 = spawner.spawn(async { 1 + 1 }).await;

    let mut relocated = spawner.clone();
    relocated.relocate(Some(&thread0), &thread1);
    let _on_thread1 = relocated.spawn(async { 1 + 1 }).await;
}

/// Scheduler data relocated by the [`ThreadAware`] system.
#[derive(Clone)]
struct Scheduler {
    threads: [Thread; 2],
    current: Thread,
}

impl Scheduler {
    fn new(threads: [Thread; 2]) -> Self {
        let current = threads[0].clone();
        Self { threads, current }
    }

    fn threads(&self) -> &[Thread] {
        &self.threads
    }

    fn caption(&self) -> String {
        format!("Scheduler ({:?})", self.current)
    }
}

impl ThreadAware for Scheduler {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.current = self
            .threads
            .iter()
            .find(|thread| *thread == destination)
            .cloned()
            .expect("destination must be one of the threads owned by Scheduler");
    }
}

impl SpawnCustom for Scheduler {
    fn spawn(&self, task: BoxedFuture) {
        println!("{}: executing", self.caption());
        tokio::spawn(task);
    }

    fn spawn_anywhere(&self, task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
        self.spawn(task.call_once());
    }

    fn spawn_blocking(&self, task: BoxedBlockingTask) {
        println!("{}: executing blocking", self.caption());
        tokio::task::spawn_blocking(task);
    }
}
