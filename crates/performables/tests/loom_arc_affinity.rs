// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Loom model of the affinity publication protocol used by `performables::arc`.
//!
//! This models the synchronization algorithm rather than the production type,
//! because Loom cannot instrument `std::sync::RwLock`. A relocation checks for
//! an existing destination, releases the lock before materializing, then
//! reacquires the lock and either publishes its value or adopts the winner.

#![cfg(loom)]
#![allow(clippy::unwrap_used, reason = "poisoning requires a prior model failure")]

use loom::sync::atomic::{AtomicUsize, Ordering};
use loom::sync::{Arc, RwLock};
use loom::thread;

type Slots = Arc<RwLock<[Option<Arc<usize>>; 2]>>;

fn relocate(slots: &Slots, destination: usize, next_id: &AtomicUsize) -> Arc<usize> {
    let existing = {
        let slots = slots.read().unwrap();
        slots[destination].as_ref().map(Arc::clone)
    };
    if let Some(value) = existing {
        return value;
    }

    // Acquiring the same lock while materializing proves that user code runs
    // outside both the read and write critical sections.
    let candidate = {
        let _materializer_probe = slots.read().unwrap();
        Arc::new(next_id.fetch_add(1, Ordering::Relaxed))
    };

    let mut slots = slots.write().unwrap();
    if let Some(value) = slots[destination].as_ref() {
        Arc::clone(value)
    } else {
        slots[destination] = Some(Arc::clone(&candidate));
        candidate
    }
}

#[test]
fn racing_relocations_to_one_destination_adopt_one_value() {
    loom::model(|| {
        let slots = Arc::new(RwLock::new([None, None]));
        let next_id = Arc::new(AtomicUsize::new(0));
        let left = {
            let slots = Arc::clone(&slots);
            let next_id = Arc::clone(&next_id);
            thread::spawn(move || relocate(&slots, 0, &next_id))
        };
        let right = {
            let slots = Arc::clone(&slots);
            let next_id = Arc::clone(&next_id);
            thread::spawn(move || relocate(&slots, 0, &next_id))
        };

        let left = left.join().unwrap();
        let right = right.join().unwrap();
        let stored = slots.read().unwrap();

        assert!(Arc::ptr_eq(&left, &right));
        assert!(Arc::ptr_eq(stored[0].as_ref().unwrap(), &left));
        assert!(stored[1].is_none());
    });
}

#[test]
fn racing_relocations_to_different_destinations_publish_both_values() {
    loom::model(|| {
        let slots = Arc::new(RwLock::new([None, None]));
        let next_id = Arc::new(AtomicUsize::new(0));
        let left = {
            let slots = Arc::clone(&slots);
            let next_id = Arc::clone(&next_id);
            thread::spawn(move || relocate(&slots, 0, &next_id))
        };
        let right = {
            let slots = Arc::clone(&slots);
            let next_id = Arc::clone(&next_id);
            thread::spawn(move || relocate(&slots, 1, &next_id))
        };

        let left = left.join().unwrap();
        let right = right.join().unwrap();
        let stored = slots.read().unwrap();

        assert!(!Arc::ptr_eq(&left, &right));
        assert!(Arc::ptr_eq(stored[0].as_ref().unwrap(), &left));
        assert!(Arc::ptr_eq(stored[1].as_ref().unwrap(), &right));
    });
}
