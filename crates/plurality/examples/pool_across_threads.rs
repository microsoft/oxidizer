// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sharing a pool between threads, and sending pooled values across them.
//!
//! A pool allocates on one thread at a time, so it hands the choice of lock to
//! the caller rather than embedding one. Handles are independent of that
//! choice: each carries its own thread bounds, and the detachable ones may be
//! dropped on a thread that never touched the pool.
//!
//! Run with `cargo run --example pool_across_threads`.

#![expect(clippy::unwrap_used, reason = "example code")]

use std::rc::Rc;
use std::sync::Mutex;
use std::thread;

use plurality::{Arc, Box, MultiPool, Pool};

#[derive(Debug)]
struct Job {
    id: u32,
    payload: [u64; 4],
}

fn main() {
    // A `Mutex` makes the allocation path shared. The pool itself imposes no
    // locking policy, so a single-threaded caller pays for none. A multi pool
    // is used here so one lock covers every type the threads allocate.
    let pool = Mutex::new(MultiPool::new());

    let results: Vec<(u32, u64)> = thread::scope(|scope| {
        // Every worker must be spawned before any is joined, or they run one
        // at a time.
        let workers: Vec<_> = (0..4)
            .map(|id| {
                let pool = &pool;
                scope.spawn(move || {
                    // Only the allocation needs the lock. The handle outlives
                    // the guard, so the work below runs uncontended.
                    let mut job = pool.lock().unwrap().alloc_box(Job {
                        id,
                        payload: [u64::from(id); 4],
                    });

                    job.payload[0] += 1;
                    let total = job.payload.iter().sum::<u64>();

                    // Returning the slot takes no lock, so reclamation runs in
                    // parallel with whatever the other threads are doing.
                    drop(job);
                    (id, total)
                })
            })
            .collect();

        workers.into_iter().map(|worker| worker.join().unwrap()).collect()
    });

    for (id, total) in &results {
        println!("job {id} summed to {total}");
    }
    println!("live after workers finished = {}", pool.lock().unwrap().len());

    // Allocation and destruction need not happen on the same thread. A `Box`
    // is `Send` whenever its value is, so it can be handed to a worker that
    // consumes it and releases the slot from there.
    let parcel: Box<Job> = pool.lock().unwrap().alloc_box(Job { id: 99, payload: [7; 4] });
    let carried = thread::spawn(move || {
        let id = parcel.id;
        drop(parcel);
        id
    })
    .join()
    .unwrap();
    println!("job {carried} was allocated here and destroyed elsewhere");

    // A shared value needs `Arc`, whose thread bounds follow the value's own.
    let config = pool.lock().unwrap().alloc_arc(String::from("shared config"));
    let observers: Vec<_> = (0..3)
        .map(|worker| {
            let config = Arc::clone(&config);
            thread::spawn(move || format!("worker {worker} read {:?}", *config))
        })
        .collect();

    for observer in observers {
        println!("{}", observer.join().unwrap());
    }

    // A pool owns no values and offers no way to reach one, so its own thread
    // mobility does not depend on the element type. This pool's values may
    // never cross a thread boundary, yet the pool moves to another thread and
    // allocates there.
    let local_values = Pool::<Rc<u32>>::new();
    let computed = thread::spawn(move || {
        let value = local_values.alloc_box(Rc::new(21));
        **value * 2
    })
    .join()
    .unwrap();
    println!("a pool of non-Send values allocated on another thread: {computed}");
}
