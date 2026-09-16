// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded batched-D protocol models; not an execution of the full allocator.
//! The reviewed singleton and expected-negative models remain in loom_tests.

use loom::sync::Arc;
use loom::sync::atomic::{AtomicUsize, Ordering};
use loom::thread;

struct Counts {
    frees: [AtomicUsize; 2],
    drained: [AtomicUsize; 2],
    pending: AtomicUsize,
    progress: AtomicUsize,
}

impl Counts {
    fn new() -> Self {
        Self {
            frees: [AtomicUsize::new(0), AtomicUsize::new(0)],
            drained: [AtomicUsize::new(0), AtomicUsize::new(0)],
            pending: AtomicUsize::new(0),
            progress: AtomicUsize::new(0),
        }
    }

    fn begin(&self, slot: usize) {
        self.progress.fetch_add(1, Ordering::Relaxed);
        self.frees[slot].fetch_add(1, Ordering::Relaxed);
        self.pending.fetch_add(1, Ordering::Relaxed);
    }

    fn finish(&self) {
        assert_ne!(self.progress.fetch_sub(1, Ordering::Relaxed), 0);
    }

    fn drain(&self, slot: usize, count: usize) {
        assert!(self.pending.fetch_sub(count, Ordering::Relaxed) >= count);
        self.drained[slot].fetch_add(count, Ordering::Relaxed);
    }

    fn totals(&self) -> (usize, usize, usize, usize) {
        (
            fold(&self.frees),
            fold(&self.drained),
            self.pending.load(Ordering::Relaxed),
            self.progress.load(Ordering::Relaxed),
        )
    }
}

fn fold(counters: &[AtomicUsize; 2]) -> usize {
    counters
        .iter()
        .fold(0usize, |sum, counter| sum.wrapping_add(counter.load(Ordering::Relaxed)))
}

#[test]
fn batched_drain_after_release_cas_chain_preserves_conservation() {
    loom::model(|| {
        let counts = Arc::new(Counts::new());
        let head = Arc::new(AtomicUsize::new(0));
        let payload = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        let writer_counts = Arc::clone(&counts);
        let writer_head = Arc::clone(&head);
        let writer_payload = Arc::clone(&payload);
        let writer = thread::spawn(move || {
            // Two admitted frees, linked before an owner can observe the chain.
            // Successful publication remains Release CAS, never a plain store.
            writer_counts.begin(0);
            writer_payload[0].store(41, Ordering::Relaxed);
            writer_head.compare_exchange(0, 1, Ordering::Release, Ordering::Relaxed).unwrap();
            writer_counts.finish();
            writer_counts.begin(1);
            writer_payload[1].store(42, Ordering::Relaxed);
            writer_head.compare_exchange(1, 2, Ordering::Release, Ordering::Relaxed).unwrap();
            writer_counts.finish();
        });
        // A relaxed observation only selects the bounded two-node case; the
        // Acquire detachment, not this observation, publishes both payloads.
        if head.load(Ordering::Relaxed) == 2 {
            assert_eq!(head.swap(0, Ordering::Acquire), 2);
            assert_eq!((payload[0].load(Ordering::Relaxed), payload[1].load(Ordering::Relaxed)), (41, 42));
            counts.drain(1, 2);
        }
        writer.join().unwrap();
        if head.swap(0, Ordering::Acquire) == 2 {
            assert_eq!((payload[0].load(Ordering::Relaxed), payload[1].load(Ordering::Relaxed)), (41, 42));
            counts.drain(1, 2);
        }
        assert_eq!(counts.totals(), (2, 2, 0, 0));
    });
}

#[test]
fn colliding_batched_drains_preserve_modulo_blocks() {
    loom::model(|| {
        let drained = Arc::new([AtomicUsize::new(usize::MAX - 1), AtomicUsize::new(0)]);
        let threads: Vec<_> = [3, 5]
            .into_iter()
            .map(|count| {
                let drained = Arc::clone(&drained);
                thread::spawn(move || {
                    drained[0].fetch_add(count, Ordering::Relaxed);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(fold(&drained), 6);
    });
}

#[test]
fn distributed_batched_drains_preserve_wrapping_fold() {
    loom::model(|| {
        let drained = Arc::new([AtomicUsize::new(usize::MAX - 1), AtomicUsize::new(0)]);
        let threads: Vec<_> = [3, 5]
            .into_iter()
            .enumerate()
            .map(|(slot, count)| {
                let drained = Arc::clone(&drained);
                thread::spawn(move || {
                    drained[slot].fetch_add(count, Ordering::Relaxed);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(fold(&drained), 6);
    });
}

#[test]
fn repeated_batched_drain_folds_do_not_regress_without_wrap() {
    loom::model(|| {
        let drained = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
        let writer_drained = Arc::clone(&drained);
        let writer = thread::spawn(move || {
            writer_drained[0].fetch_add(3, Ordering::Relaxed);
            writer_drained[1].fetch_add(5, Ordering::Relaxed);
        });
        let first = fold(&drained);
        let second = fold(&drained);
        assert!(first <= second && second <= 8);
        writer.join().unwrap();
        assert_eq!(fold(&drained), 8);
    });
}
