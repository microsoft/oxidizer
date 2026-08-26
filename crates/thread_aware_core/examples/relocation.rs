// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Creating places and relocating a value between them.
//!
//! Run with `cargo run -p thread_aware_core --example relocation`.

#![allow(missing_docs, reason = "Example code")]
#![allow(clippy::print_stdout, reason = "Example code")]

use std::thread;

use thread_aware_core::{NumaNode, Origin, Place, ThreadAware};

/// A sample value that remembers which thread it currently runs on.
struct Worker {
    thread: Option<thread::ThreadId>,
}

impl ThreadAware for Worker {
    // `relocate` is limited to bounded local work, so this only records the destination.
    // Reporting happens in `main`, outside the callback.
    fn relocate(&mut self, _source: Option<&Place>, destination: &Place) {
        self.thread = Some(destination.thread());
    }
}

fn main() {
    // Manually create two places, one per thread, on the same NUMA node.
    let here = thread::current().id();
    let there = thread::spawn(|| thread::current().id())
        .join()
        .expect("the spawned thread cannot panic");

    let origin = Origin::from(1);
    let first = Place::new(origin, here, NumaNode::from(0));
    let second = Place::new(origin, there, NumaNode::from(0));

    // Relocate a sample object between them.
    let mut worker = Worker { thread: None };

    worker.relocate(None, &first); // initial placement; the previous place is unknown
    println!("placed on thread {:?}", worker.thread);

    worker.relocate(Some(&first), &second); // migrate from the first place to the second
    println!("relocated to thread {:?}", worker.thread);
}
