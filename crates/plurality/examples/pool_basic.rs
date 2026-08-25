// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The handle flavors a typed pool hands out, and what distinguishes them.
//!
//! Run with `cargo run --example pool_basic`.

use plurality::{Arc, Box, Pool, Rc};

#[derive(Debug)]
struct Particle {
    id: u32,
    position: (f32, f32),
}

fn main() {
    let pool = Pool::<Particle>::new();

    // `alloc_box` is the workhorse: a unique owner that may leave the scope
    // the pool lives in, and may be dropped on another thread.
    let mut owned: Box<Particle> = pool.alloc_box(Particle {
        id: 1,
        position: (0.0, 0.0),
    });
    owned.position = (3.0, 4.0);
    println!("owned    = {owned:?}");

    // `alloc_arc` shares a value across threads. Cloning bumps a per-value
    // count; the value is destroyed when the last clone goes away.
    let shared: Arc<Particle> = pool.alloc_arc(Particle {
        id: 2,
        position: (1.0, 1.0),
    });
    let shared_clone = Arc::clone(&shared);
    println!("shared   = id {}, seen through a clone", shared_clone.id);

    // `alloc_rc` is the same sharing model without the atomics, for values
    // that never leave the thread that made them.
    let local: Rc<Particle> = pool.alloc_rc(Particle {
        id: 3,
        position: (2.0, 2.0),
    });
    println!("local    = {local:?}");

    // `alloc` is the cheapest: it borrows the pool, so it cannot outlive it,
    // and in exchange it skips the pool-level bookkeeping the others pay.
    let borrowed = pool.alloc(Particle {
        id: 4,
        position: (5.0, 5.0),
    });
    println!("borrowed = {borrowed:?}");

    // Skipping that bookkeeping is why `len` does not see the borrowed value:
    // it counts the handles that hold a unit of the pool-level count, which
    // is exactly the set of handles able to outlive the pool object.
    println!("live     = {} (the borrowed handle is not counted)", pool.len());

    // A pooled value keeps its address for as long as it lives, so a raw
    // pointer taken to pooled data stays good however the pool is used around
    // it.
    let address = &raw const *owned;
    drop(shared);
    drop(shared_clone);
    drop(local);
    drop(borrowed);
    println!("address stable after neighbors drop: {}", address == &raw const *owned);

    // Freed slots are reused rather than returned to the system allocator, so
    // a steady workload stops allocating once it reaches its high-water mark.
    let capacity_before = pool.capacity();
    drop(owned);
    let recycled = pool.alloc_box(Particle {
        id: 5,
        position: (9.0, 9.0),
    });
    println!(
        "id {} took over the slot just freed: {}, without growing: {}",
        recycled.id,
        &raw const *recycled == address,
        pool.capacity() == capacity_before
    );

    // A detachable handle owns its slot outright. The pool object may go away
    // while the value lives on; the memory is torn down after the last handle.
    let survivor = {
        let scratch = Pool::<Particle>::new();
        scratch.alloc_box(Particle {
            id: 6,
            position: (7.0, 7.0),
        })
    };
    println!("outlived its pool: {survivor:?}");
}
