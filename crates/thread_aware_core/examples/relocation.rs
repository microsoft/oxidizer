// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Creating `Thread` values and relocating a value between them.
//!
//! Run with `cargo run -p thread_aware_core --example relocation`.

#![allow(missing_docs, reason = "Example code")]
#![allow(clippy::print_stdout, reason = "Example code")]

use std::thread;

use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};

/// A sample value that remembers which thread it currently runs on.
struct Worker {
    thread: Option<thread::ThreadId>,
}

impl ThreadAware for Worker {
    // `relocate` is limited to bounded local work, so this only records the destination.
    // Reporting happens in `main`, outside the callback.
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.thread = Some(destination.id());
    }
}

fn main() {
    // Manually build two `Thread` values, one per OS thread, on the same NUMA node.
    let here = thread::current().id();
    let there = thread::spawn(|| thread::current().id())
        .join()
        .expect("the spawned thread cannot panic");

    let owner = Owner::new(2);
    let first = Thread::new(owner.clone(), here, NumaNode::new(0));
    let second = Thread::new(owner, there, NumaNode::new(0));

    // Relocate a sample object between them.
    let mut worker = Worker { thread: None };

    worker.relocate(None, &first); // initial placement; no previous `Thread`
    println!("placed on thread {:?}", worker.thread);

    worker.relocate(Some(&first), &second); // migrate from the first `Thread` to the second
    println!("relocated to thread {:?}", worker.thread);
}
