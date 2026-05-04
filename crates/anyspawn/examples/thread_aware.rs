// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Per-core task spawning with a custom [`SpawnCustom`] implementation.
//!
//! Demonstrates creating a spawner where each CPU core gets its own independent
//! state via the [`ThreadAware`] relocation system. A custom [`Scheduler`] type
//! carries the processor index assigned during relocation, so each core's spawn
//! function knows which core it belongs to.
//!
//! In production the data might hold a core-local work queue, metrics
//! counter, or connection pool instead of a simple index.

use anyspawn::Spawner;
use thread_aware::affinity::{pinned_affinities, MemoryAffinity, PinnedAffinity};
use thread_aware::ThreadAware;

#[tokio::main]
async fn main() {
    // Build a per-core spawner. Each core gets a Scheduler with its own
    // processor index after relocation.
    let spawner = Spawner::new_custom("per-core", Scheduler::default());

    // Before relocation the spawner uses the default (unassigned) scheduler.
    let _default = spawner.spawn(async { 1 + 1 }).await;

    // Simulate a two-node topology (1 core per NUMA node) and relocate the
    // spawner to each core. After relocation ThreadAware::relocated runs
    // with the destination core's processor index.
    let affinities = pinned_affinities(&[1, 1]);

    let mut relocated0 = spawner.clone();
    relocated0.relocated(MemoryAffinity::Unknown, affinities[0]);
    let _relocated0 = relocated0.spawn(async { 1 + 1 }).await;

    let mut relocated1 = spawner.clone();
    relocated1.relocated(MemoryAffinity::Unknown, affinities[1]);
    let _relocated1 = relocated1.spawn(async { 1 + 1 }).await;
}

/// Per-core scheduler data relocated by the [`ThreadAware`] system.
///
/// Before relocation the processor index is `None` (default instance). After
/// relocation it holds the destination core's processor index.
#[derive(Default, Clone)]
struct Scheduler(Option<usize>);

impl Scheduler {
    fn caption(&self) -> String {
        match self.0 {
            Some(id) => format!("Scheduler ({id})"),
            None => "Scheduler (default)".to_string(),
        }
    }
}

impl ThreadAware for Scheduler {
    fn relocated(&mut self, _source: MemoryAffinity, destination: PinnedAffinity) {
        self.0 = Some(destination.processor_index());
    }
}

impl SpawnCustom for Scheduler {
    fn spawn(&self, task: BoxedFuture) {
        println!("{}: executing", self.caption());
        tokio::spawn(task);
    }

    fn spawn_anywhere(&self, task: BoxedFuture) {
        self.spawn(task);
    }
}
