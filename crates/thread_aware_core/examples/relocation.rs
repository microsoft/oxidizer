// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Creating locations and relocating a value between them.
//!
//! Run with `cargo run -p thread_aware_core --example relocation`.

#![allow(missing_docs, reason = "Example code")]
#![allow(clippy::print_stdout, reason = "Example code")]

use thread_aware_core::{Core, Location, MemoryRegion, ThreadAware, Topology};

/// A sample value that remembers which core it currently runs on.
struct Worker {
    core: Option<Core>,
}

impl ThreadAware for Worker {
    fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
        self.core = Some(destination.core());
        println!("relocated from {:?} to core {:?}", source.map(Location::core), destination.core());
    }
}

fn main() {
    // Manually create two locations in the same topology.
    let topology = Topology::from(1);
    let first = Location::new(topology, Core::from(0), MemoryRegion::from(0));
    let second = Location::new(topology, Core::from(3), MemoryRegion::from(1));

    // Relocate a sample object between them.
    let mut worker = Worker { core: None };
    worker.relocate(None, &first); // initial placement; the previous location is unknown
    worker.relocate(Some(&first), &second); // migrate from the first location to the second

    println!("worker now on core {:?}", worker.core);
}
