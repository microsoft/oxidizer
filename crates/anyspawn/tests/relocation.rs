// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test code")]
#![cfg(feature = "custom")]

//! Tests for [`Spawner`] relocation behavior with [`ThreadAware`].
//!
//! Per-process spawners must be unaffected by relocation (same spawn function
//! shared everywhere). Thread-aware spawners must create a new spawn function
//! for each core and dispatch through the one associated with the destination.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyspawn::Spawner;
use thread_aware::ThreadAware;
use thread_aware::affinity::pinned_affinities;

/// Per-process spawner: relocation must not change which spawn function is used.
///
/// Both the original and relocated spawner share the same underlying closure,
/// so a shared counter captured by that closure must be incremented by both.
#[test]
fn per_process_relocation_preserves_spawn_function() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = Arc::clone(&call_count);

    let spawner = Spawner::new_custom("shared", move |fut| {
        cc.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || futures::executor::block_on(fut));
    });

    let affinities = pinned_affinities(&[2]);
    let original = spawner.clone();
    let relocated = spawner.relocated(affinities[0].into(), affinities[1]);

    let r1 = futures::executor::block_on(original.spawn(async { 1 }));
    let r2 = futures::executor::block_on(relocated.spawn(async { 2 }));

    assert_eq!(r1, 1);
    assert_eq!(r2, 2);
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "both spawners must route through the same spawn function"
    );
}

/// Thread-aware spawner: relocation must invoke the factory to create a fresh
/// spawn function for the destination core.
#[test]
fn thread_aware_relocation_invokes_factory_for_new_core() {
    // Safe to use statics: nextest runs each test in its own process.
    static FACTORY_CALLS: AtomicUsize = AtomicUsize::new(0);

    let spawner = Spawner::new_thread_aware((), |()| {
        FACTORY_CALLS.fetch_add(1, Ordering::SeqCst);
        Spawner::new_custom("per-core", |fut| {
            std::thread::spawn(move || futures::executor::block_on(fut));
        })
    });

    assert_eq!(FACTORY_CALLS.load(Ordering::SeqCst), 1, "factory called on creation");

    let affinities = pinned_affinities(&[2]);
    let original = spawner.clone();
    let relocated = spawner.relocated(affinities[0].into(), affinities[1]);

    assert_eq!(
        FACTORY_CALLS.load(Ordering::SeqCst),
        2,
        "factory must be called again for the destination core"
    );

    let r1 = futures::executor::block_on(original.spawn(async { 10 }));
    let r2 = futures::executor::block_on(relocated.spawn(async { 20 }));
    assert_eq!(r1, 10);
    assert_eq!(r2, 20);
}

/// Thread-aware spawner: after relocation, spawning must dispatch through the
/// spawn function associated with the destination core, not the source.
///
/// Each factory call assigns a unique ID to the spawn function it creates. When
/// a spawn function is invoked it records its ID in a shared log. After spawning
/// on both the original and relocated spawner, the log must contain two
/// *different* IDs, proving each dispatched through its own spawn function.
#[test]
fn thread_aware_relocated_spawner_dispatches_through_destination() {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    static DISPATCH_LOG: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());

    fn factory(_: ()) -> Spawner {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        Spawner::new_custom("per-core", move |fut| {
            DISPATCH_LOG.lock().expect("dispatch log should not be poisoned").push(id);
            std::thread::spawn(move || futures::executor::block_on(fut));
        })
    }

    let spawner = Spawner::new_thread_aware((), factory);

    let affinities = pinned_affinities(&[2]);
    let original = spawner.clone();
    let relocated = spawner.relocated(affinities[0].into(), affinities[1]);

    futures::executor::block_on(original.spawn(async {}));
    futures::executor::block_on(relocated.spawn(async {}));

    let log = DISPATCH_LOG.lock().expect("dispatch log should not be poisoned");
    assert_eq!(log.len(), 2, "both spawners should have dispatched exactly once");
    assert_ne!(
        log[0], log[1],
        "original (id={}) and relocated (id={}) must use different spawn functions",
        log[0], log[1]
    );
}
