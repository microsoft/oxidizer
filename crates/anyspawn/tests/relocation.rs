// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, clippy::items_after_statements, reason = "test code")]
#![cfg(not(miri))] // miri does not support OS threads and CPU affinity helpers

//! Tests for [`Spawner`] relocation behavior with [`ThreadAware`].
//!
//! Per-process spawners with a no-op [`ThreadAware`] are unaffected by
//! relocation. Thread-aware spawners (custom [`SpawnCustom`] impls) create
//! per-core state through `clone` + `relocate`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyspawn::{BoxedFuture, SpawnCustom, Spawner};
use thread_aware::ThreadAware;
use thread_aware::affinity::{self, pinned_affinities};
use thread_aware::closure::ThreadAwareAsyncFnOnce;

/// Per-process spawner: relocation must not change which spawn function is used.
///
/// Both the original and relocated spawner share the same underlying state,
/// so a shared counter must be incremented by both.
#[test]
fn per_process_relocation_preserves_spawn_function() {
    let call_count = Arc::new(AtomicUsize::new(0));

    #[derive(Clone)]
    struct CountingSpawner(Arc<AtomicUsize>);

    impl ThreadAware for CountingSpawner {
        fn relocate(&mut self, _: Option<affinity::Affinity>, _: affinity::Affinity) {}
    }

    impl SpawnCustom for CountingSpawner {
        fn spawn(&self, task: BoxedFuture) {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || futures::executor::block_on(task));
        }

        fn spawn_anywhere(&self, task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
            self.spawn(task.call_once());
        }
    }

    let spawner = Spawner::new_custom("shared", CountingSpawner(Arc::clone(&call_count)));

    let affinities = pinned_affinities(&[2]);
    let original = spawner.clone();
    let mut spawner = spawner;
    spawner.relocate(Some(affinities[0]), affinities[1]);

    let r1 = futures::executor::block_on(original.spawn(async { 1 }));
    let r2 = futures::executor::block_on(spawner.spawn(async { 2 }));

    assert_eq!(r1, 1);
    assert_eq!(r2, 2);
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "both spawners must route through the same spawn function"
    );
}

/// Thread-aware spawner: relocation must invoke `relocate` to create fresh
/// per-core state.
#[test]
fn thread_aware_relocation_invokes_relocated_for_new_core() {
    static RELOCATE_CALLS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone)]
    struct CountingSpawner;

    impl ThreadAware for CountingSpawner {
        fn relocate(&mut self, _: Option<affinity::Affinity>, _: affinity::Affinity) {
            RELOCATE_CALLS.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl SpawnCustom for CountingSpawner {
        fn spawn(&self, task: BoxedFuture) {
            std::thread::spawn(move || futures::executor::block_on(task));
        }

        fn spawn_anywhere(&self, task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
            let fut = task.call_once();
            std::thread::spawn(move || futures::executor::block_on(fut));
        }
    }

    let spawner = Spawner::new_custom("per-core", CountingSpawner);

    let before = RELOCATE_CALLS.load(Ordering::SeqCst);

    let affinities = pinned_affinities(&[2]);
    let original = spawner.clone();
    let mut spawner = spawner;
    spawner.relocate(Some(affinities[0]), affinities[1]);

    assert!(
        RELOCATE_CALLS.load(Ordering::SeqCst) > before,
        "relocated must be called for the destination core"
    );

    let r1 = futures::executor::block_on(original.spawn(async { 10 }));
    let r2 = futures::executor::block_on(spawner.spawn(async { 20 }));
    assert_eq!(r1, 10);
    assert_eq!(r2, 20);
}

/// Thread-aware spawner: after relocation, spawning must dispatch through the
/// spawn function associated with the destination core, not the source.
///
/// Each instance gets a unique ID on construction via `relocate`. When a spawn
/// function is invoked it records its ID in a shared log. After spawning on both
/// the original and relocated spawner, the log must contain two *different* IDs.
#[test]
fn thread_aware_relocated_spawner_dispatches_through_destination() {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
    static DISPATCH_LOG: Mutex<Vec<usize>> = Mutex::new(Vec::new());

    #[derive(Clone)]
    struct IdSpawner {
        id: usize,
    }

    impl ThreadAware for IdSpawner {
        fn relocate(&mut self, _: Option<affinity::Affinity>, _: affinity::Affinity) {
            self.id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl SpawnCustom for IdSpawner {
        fn spawn(&self, task: BoxedFuture) {
            DISPATCH_LOG.lock().expect("dispatch log should not be poisoned").push(self.id);
            std::thread::spawn(move || futures::executor::block_on(task));
        }

        fn spawn_anywhere(&self, task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
            self.spawn(task.call_once());
        }
    }

    let spawner = Spawner::new_custom("per-core", IdSpawner { id: 0 });

    let affinities = pinned_affinities(&[2]);
    let original = spawner.clone();
    let mut spawner = spawner;
    spawner.relocate(Some(affinities[0]), affinities[1]);

    futures::executor::block_on(original.spawn(async {}));
    futures::executor::block_on(spawner.spawn(async {}));

    let log = DISPATCH_LOG.lock().expect("dispatch log should not be poisoned");
    assert_eq!(log.len(), 2, "both spawners should have dispatched exactly once");
    assert_ne!(
        log[0], log[1],
        "original (id={}) and relocated (id={}) must use different spawn functions",
        log[0], log[1]
    );
}

/// Verify that `spawn_anywhere` relocates the task data before execution.
///
/// Uses a custom spawner whose `spawn_anywhere` relocates the task to a
/// different affinity before calling `call_once`, exercising
/// `SpawnAnywhereTask::relocate`.
#[test]
fn spawn_anywhere_relocates_task_data() {
    use std::sync::atomic::AtomicBool;

    static DATA_WAS_RELOCATED: AtomicBool = AtomicBool::new(false);

    #[derive(Clone)]
    struct Tracker(bool);

    impl ThreadAware for Tracker {
        fn relocate(&mut self, _: Option<affinity::Affinity>, _: affinity::Affinity) {
            self.0 = true;
            DATA_WAS_RELOCATED.store(true, Ordering::SeqCst);
        }
    }

    /// A spawner that relocates the task before execution.
    #[derive(Clone)]
    struct RelocatingSpawner;

    impl ThreadAware for RelocatingSpawner {
        fn relocate(&mut self, _: Option<affinity::Affinity>, _: affinity::Affinity) {}
    }

    impl SpawnCustom for RelocatingSpawner {
        fn spawn(&self, task: BoxedFuture) {
            std::thread::spawn(move || futures::executor::block_on(task));
        }

        fn spawn_anywhere(&self, mut task: Box<dyn ThreadAwareAsyncFnOnce<()>>) {
            let affinities = pinned_affinities(&[2]);
            task.relocate(Some(affinities[0]), affinities[1]);
            self.spawn(task.call_once());
        }
    }

    let spawner = Spawner::new_custom("relocating", RelocatingSpawner);
    let handle = spawner.spawn_anywhere(Tracker(false), |t| async move {
        assert!(t.0, "data must have been relocated before call_once");
    });

    futures::executor::block_on(handle);
    assert!(
        DATA_WAS_RELOCATED.load(Ordering::SeqCst),
        "SpawnAnywhereTask must forward relocate to captured data"
    );
}
