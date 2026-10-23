// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Abstract concurrency **algorithm sketches** — *not* a model-check of the
//! production lock.
//!
//! These Loom models deliberately implement a **different** interning algorithm
//! than production: a textbook `std`-style *read, drop, write, then re-check*
//! sequence. Production (`Shard::intern`) instead holds a single `parking_lot`
//! **upgradable-read guard** continuously and **atomically upgrades** it to the
//! write lock on a miss, so there is no read-drop gap and no re-check is needed.
//!
//! They diverge because **Loom cannot instrument `parking_lot`**, and
//! `loom::sync::RwLock` has no upgradable-guard primitive, so the production
//! transition simply cannot be expressed here. What these tests validate is the
//! *abstract* dedup and snapshot invariants (racing equal strings collapse to one
//! handle; distinct strings stay resolvable; a concurrent snapshot observes a
//! consistent prefix) under an interleaving-exhaustive model — they are useful
//! design sketches, and must **not** be read as evidence that the production
//! upgradable-lock implementation is model-checked.

#![cfg(loom)]
#![allow(clippy::unwrap_used, reason = "poisoning cannot occur in this Loom model without a prior failure")]

use loom::sync::{Arc, RwLock};
use loom::thread;

/// The **sketch** interning algorithm exercised by these models: a `std`-style
/// read-then-drop, write, re-check sequence. This is intentionally *not* how
/// production interns — production holds one `parking_lot` upgradable-read guard
/// and atomically upgrades it, avoiding the read-drop gap that makes the re-check
/// below necessary. See the module docs.
fn intern(shard: &RwLock<Vec<&'static str>>, value: &'static str) -> usize {
    if let Some(index) = shard.read().unwrap().iter().position(|stored| *stored == value) {
        return index;
    }

    let mut write = shard.write().unwrap();
    if let Some(index) = write.iter().position(|stored| *stored == value) {
        return index;
    }
    let index = write.len();
    write.push(value);
    index
}

#[test]
fn racing_equal_strings_get_one_handle() {
    loom::model(|| {
        let shard = Arc::new(RwLock::new(Vec::new()));
        let left = {
            let shard = Arc::clone(&shard);
            thread::spawn(move || intern(&shard, "same"))
        };
        let right = {
            let shard = Arc::clone(&shard);
            thread::spawn(move || intern(&shard, "same"))
        };

        assert_eq!(left.join().unwrap(), right.join().unwrap());
        assert_eq!(*shard.read().unwrap(), ["same"]);
    });
}

#[test]
fn racing_distinct_and_equal_strings_remain_resolvable() {
    loom::model(|| {
        let shard = Arc::new(RwLock::new(Vec::new()));
        let left = {
            let shard = Arc::clone(&shard);
            thread::spawn(move || {
                let alpha = intern(&shard, "alpha");
                let common = intern(&shard, "shared");
                (alpha, common)
            })
        };
        let right = {
            let shard = Arc::clone(&shard);
            thread::spawn(move || {
                let beta = intern(&shard, "beta");
                let common = intern(&shard, "shared");
                (beta, common)
            })
        };

        let (alpha, left_shared) = left.join().unwrap();
        let (beta, right_shared) = right.join().unwrap();
        let strings = shard.read().unwrap();

        assert_ne!(alpha, beta);
        assert_eq!(left_shared, right_shared);
        assert_eq!(strings[alpha], "alpha");
        assert_eq!(strings[beta], "beta");
        assert_eq!(strings[left_shared], "shared");
        assert_eq!(strings.len(), 3);
    });
}

#[test]
fn snapshot_is_a_consistent_prefix_during_interning() {
    loom::model(|| {
        let shard = Arc::new(RwLock::new(vec!["first"]));
        let writer = {
            let shard = Arc::clone(&shard);
            thread::spawn(move || intern(&shard, "second"))
        };
        let snapshotter = {
            let shard = Arc::clone(&shard);
            thread::spawn(move || shard.read().unwrap().clone())
        };

        let second = writer.join().unwrap();
        let snapshot = snapshotter.join().unwrap();
        let final_state = shard.read().unwrap();

        assert_eq!(final_state[second], "second");
        assert!(snapshot == ["first"] || snapshot == ["first", "second"]);
    });
}
