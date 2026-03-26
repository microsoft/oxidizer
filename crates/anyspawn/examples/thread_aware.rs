// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Per-core task spawning with [`Spawner::new_thread_aware`].
//!
//! Demonstrates creating a spawner where each CPU core gets its own independent
//! spawn function. A custom [`Scheduler`] type carries the processor index
//! assigned by the [`ThreadAware`] relocation system, so each core's spawn
//! function knows which core it belongs to.
//!
//! In production the data argument might hold a core-local work queue, metrics
//! counter, or connection pool instead of a simple index.

use anyspawn::Spawner;
use thread_aware::ThreadAware;
use thread_aware::affinity::{MemoryAffinity, PinnedAffinity, pinned_affinities};

#[tokio::main]
async fn main() {
    // Build a per-core spawner backed by Tokio. The factory receives a
    // `Scheduler` that has been relocated to the target core, so each core's
    // spawn function prints a core-specific caption.
    let spawner = Spawner::new_thread_aware(Scheduler::default(), |scheduler| {
        anyspawn::CustomSpawnerBuilder::tokio()
            .layer(move |fut, inner| {
                println!("{}: executing", scheduler.caption());
                inner(fut);
            })
            .build()
    });

    // Before relocation the spawner uses the default (unassigned) scheduler.
    let _default = spawner.spawn(async { 1 + 1 }).await;

    // Simulate a two-node topology (1 core per NUMA node) and relocate the
    // spawner to each core. After relocation the factory runs again with a
    // `Scheduler` whose processor index matches the destination core.
    let affinities = pinned_affinities(&[1, 1]);

    let _relocated0 = spawner
        .clone()
        .relocated(MemoryAffinity::Unknown, affinities[0])
        .spawn(async { 1 + 1 })
        .await;

    let _relocated1 = spawner
        .clone()
        .relocated(MemoryAffinity::Unknown, affinities[1])
        .spawn(async { 1 + 1 })
        .await;
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
    fn relocated(self, _source: MemoryAffinity, destination: PinnedAffinity) -> Self {
        Self(Some(destination.processor_index()))
    }
}
