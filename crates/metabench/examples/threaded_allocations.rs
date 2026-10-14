// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic process-wide allocation accounting with prestarted threads.

#![expect(clippy::unwrap_used, reason = "process fixture failures provide sufficient context")]

use std::hint::black_box;
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle};

// Return ownership so final thread allocations and joins happen after collection.
struct Workers {
    start: Arc<Barrier>,
    complete: Arc<Barrier>,
    finish: Arc<Barrier>,
    threads: [Option<JoinHandle<()>>; 2],
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.finish.wait();
        for thread in &mut self.threads {
            thread.take().unwrap().join().unwrap();
        }
    }
}

fn prepare() -> Workers {
    let ready = Arc::new(Barrier::new(3));
    let start = Arc::new(Barrier::new(3));
    let complete = Arc::new(Barrier::new(3));
    let finish = Arc::new(Barrier::new(3));
    let threads = [37, 83].map(|size| {
        let ready = Arc::clone(&ready);
        let start = Arc::clone(&start);
        let complete = Arc::clone(&complete);
        let finish = Arc::clone(&finish);
        Some(thread::spawn(move || {
            // Initialize each thread's tracker and finish startup before collection.
            drop(black_box(vec![1_u8; 2048]));
            ready.wait();
            start.wait();
            drop(black_box(vec![2_u8; black_box(size)]));
            drop(black_box(vec![3_u8; black_box(19)]));
            complete.wait();
            finish.wait();
            drop(black_box(vec![4_u8; 4096]));
        }))
    });
    ready.wait();
    Workers {
        start,
        complete,
        finish,
        threads,
    }
}

#[metabench::benchmark(THREADED, "threads", "allocations", gungraun_setup = prepare)]
fn allocate_on_threads(workers: Workers) -> Workers {
    workers.start.wait();
    workers.complete.wait();
    workers
}

#[expect(clippy::panic, reason = "process fixture fails if a native probe executes Criterion registration")]
fn criterion_benchmarks(_: &mut criterion::Criterion) {
    panic!("the process fixture only runs native allocation probes");
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [THREADED],);
