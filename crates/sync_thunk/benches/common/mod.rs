// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![cfg(not(loom))]

//! Shared dispatch scenarios for the `sync_thunk` benchmarks.
//!
//! Both `criterion_dispatch` and `gungraun_dispatch` exercise the same four
//! scenarios so their results can be diffed directly:
//!
//! 1. `thunk_void`            — `#[thunk]` call with no arguments
//! 2. `thunk_arg_u64`         — `#[thunk]` call with one `u64` argument
//! 3. `spawn_blocking_void`   — bare `tokio::task::spawn_blocking(|| 0u64)`
//! 4. `spawn_blocking_arg_u64`— realistic `Arc<Self>::clone` + `move ||` with arg
//!
//! Each scenario performs `N` sequential awaits inside a single
//! `Runtime::block_on`. The runtime, the `Thunker`, the `Service`, and the
//! `Arc<Service>` are all built once during setup so the timed region only
//! includes per-call dispatch.
//!
//! The bench bodies intentionally return a trivial constant. We are measuring
//! *dispatch overhead*, not the cost of the work itself.

#![allow(dead_code, reason = "shared module used by multiple bench binaries")]

use std::hint::black_box;
use std::sync::Arc;

use sync_thunk::{Thunker, thunk};
use tokio::runtime::{Builder, Runtime};

/// Number of awaits per timed iteration. Picked high enough to amortise
/// `Runtime::block_on` entry/exit and low enough to keep gungraun runs short.
pub const N: usize = 1_000;

/// Test fixture exposing both `#[thunk]` methods and the equivalent
/// `spawn_blocking` variants on the same service object.
pub struct Service {
    thunker: Thunker,
}

impl Service {
    pub fn new() -> Self {
        Self {
            // Single worker keeps the dispatch path deterministic. Adding
            // workers does not help under Valgrind (threads are serialised)
            // and only adds wake-up nondeterminism under criterion.
            thunker: Thunker::builder().max_thread_count(1).build(),
        }
    }

    #[thunk(from = me.thunker)]
    pub async fn thunk_void(me: Arc<Self>) -> u64 {
        let _keep = &me;
        black_box(0_u64)
    }

    #[thunk(from = me.thunker)]
    pub async fn thunk_arg_u64(me: Arc<Self>, x: u64) -> u64 {
        let _keep = &me;
        black_box(x.wrapping_add(1))
    }

    /// Mirror of `thunk_void` using `tokio::task::spawn_blocking` directly.
    ///
    /// Takes `Arc<Self>` to match the realistic call site: the closure is
    /// `'static`, so it cannot borrow `&self`.
    pub async fn spawn_blocking_void(self: &Arc<Self>) -> u64 {
        let me = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            // Touch `me` so the move-capture cannot be optimised away.
            let _keep = &me;
            black_box(0_u64)
        })
        .await
        .expect("worker panicked")
    }

    /// Mirror of `thunk_arg_u64` using `tokio::task::spawn_blocking` directly.
    pub async fn spawn_blocking_arg_u64(self: &Arc<Self>, x: u64) -> u64 {
        let me = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let _keep = &me;
            black_box(x.wrapping_add(1))
        })
        .await
        .expect("worker panicked")
    }
}

/// All state a bench iteration needs. Built once, reused across iterations.
pub struct Fixture {
    pub runtime: Runtime,
    pub service: Arc<Service>,
}

impl Fixture {
    pub fn new() -> Self {
        // current-thread runtime: deterministic polling order, no work-stealing.
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");
        let service = Arc::new(Service::new());

        // Warm-up: prime the worker thread and channel slots so the first
        // timed call does not pay the cold-start cost. Mirrors multitude's
        // `with_capacity_*` warm-up of arena chunks.
        runtime.block_on(async {
            let _ = Service::thunk_void(Arc::clone(&service)).await;
            let _ = Service::thunk_arg_u64(Arc::clone(&service), 1).await;
            let _ = service.spawn_blocking_void().await;
            let _ = service.spawn_blocking_arg_u64(1).await;
        });

        Self { runtime, service }
    }
}

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}

// ===== Bench bodies =====
//
// Each runs N sequential awaits inside a single `block_on`. The black_box on
// the accumulator prevents the optimiser from collapsing the loop.

pub fn run_thunk_void(fixture: &Fixture) {
    let svc = &fixture.service;
    fixture.runtime.block_on(async {
        let mut acc: u64 = 0;
        for _ in 0..N {
            acc = acc.wrapping_add(Service::thunk_void(Arc::clone(svc)).await);
        }
        black_box(acc);
    });
}

pub fn run_thunk_arg_u64(fixture: &Fixture) {
    let svc = &fixture.service;
    fixture.runtime.block_on(async {
        let mut acc: u64 = 0;
        for i in 0..N as u64 {
            acc = acc.wrapping_add(Service::thunk_arg_u64(Arc::clone(svc), black_box(i)).await);
        }
        black_box(acc);
    });
}

pub fn run_spawn_blocking_void(fixture: &Fixture) {
    let svc = &fixture.service;
    fixture.runtime.block_on(async {
        let mut acc: u64 = 0;
        for _ in 0..N {
            acc = acc.wrapping_add(svc.spawn_blocking_void().await);
        }
        black_box(acc);
    });
}

pub fn run_spawn_blocking_arg_u64(fixture: &Fixture) {
    let svc = &fixture.service;
    fixture.runtime.block_on(async {
        let mut acc: u64 = 0;
        for i in 0..N as u64 {
            acc = acc.wrapping_add(svc.spawn_blocking_arg_u64(black_box(i)).await);
        }
        black_box(acc);
    });
}
