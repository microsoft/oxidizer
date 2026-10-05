// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Small models mirror the immediate RMW / independent folding protocol.
//! Production uses std atomics; these are not a full-allocator Loom execution.

use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use loom::thread;

struct Counts {
    frees: [AtomicUsize; 2],
    drained: [AtomicUsize; 2],
    pending: AtomicUsize,
    progress: AtomicUsize,
}

impl Counts {
    fn new(initial: [usize; 2]) -> Self {
        Self {
            frees: initial.map(AtomicUsize::new),
            drained: [AtomicUsize::new(0), AtomicUsize::new(0)],
            pending: AtomicUsize::new(0),
            progress: AtomicUsize::new(0),
        }
    }

    fn frees(&self) -> usize {
        self.frees
            .iter()
            .fold(0usize, |sum, counter| sum.wrapping_add(counter.load(Ordering::Relaxed)))
    }

    fn drain(&self, slot: usize) {
        assert_ne!(self.pending.fetch_sub(1, Ordering::Relaxed), 0);
        self.drained[slot].fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn colliding_writers_preserve_completed_events() {
    loom::model(|| {
        let counts = Arc::new(Counts::new([0, 0]));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let counts = Arc::clone(&counts);
                thread::spawn(move || {
                    counts.frees[0].fetch_add(1, Ordering::Relaxed);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(counts.frees(), 2);
    });
}

#[test]
fn repeated_relaxed_observations_do_not_regress_without_wrap() {
    loom::model(|| {
        let counts = Arc::new(Counts::new([0, 0]));
        let writer_counts = Arc::clone(&counts);
        let writer = thread::spawn(move || {
            writer_counts.frees[0].fetch_add(1, Ordering::Relaxed);
            writer_counts.frees[1].fetch_add(1, Ordering::Relaxed);
        });
        let first = counts.frees();
        let second = counts.frees();
        assert!(first <= second && second <= 2);
        writer.join().unwrap();
        assert_eq!(counts.frees(), 2);
    });
}

#[test]
fn synchronized_completed_events_are_visible_in_every_slot() {
    loom::model(|| {
        let counts = Arc::new(Counts::new([0, 0]));
        let ready = Arc::new(AtomicBool::new(false));
        let writer_counts = Arc::clone(&counts);
        let writer_ready = Arc::clone(&ready);
        let writer = thread::spawn(move || {
            writer_counts.frees[0].fetch_add(1, Ordering::Relaxed);
            writer_counts.frees[1].fetch_add(1, Ordering::Relaxed);
            writer_ready.store(true, Ordering::Release);
        });
        if ready.load(Ordering::Acquire) {
            assert_eq!(counts.frees(), 2);
        }
        writer.join().unwrap();
        assert_eq!(counts.frees(), 2);
    });
}

#[test]
#[should_panic(expected = "relaxed notification does not publish events")]
fn rejected_unsynchronized_visibility_assumption() {
    loom::model(|| {
        let counts = Arc::new(Counts::new([0, 0]));
        let ready = Arc::new(AtomicBool::new(false));
        let writer_counts = Arc::clone(&counts);
        let writer_ready = Arc::clone(&ready);
        let writer = thread::spawn(move || {
            writer_counts.frees[0].fetch_add(1, Ordering::Relaxed);
            writer_ready.store(true, Ordering::Relaxed);
        });
        if ready.load(Ordering::Relaxed) {
            assert_eq!(counts.frees(), 1, "relaxed notification does not publish events");
        }
        writer.join().unwrap();
    });
}

fn model_overflow(initial: [usize; 2]) {
    loom::model(move || {
        let counts = Arc::new(Counts::new(initial));
        let threads: Vec<_> = (0..2)
            .map(|slot| {
                let counts = Arc::clone(&counts);
                thread::spawn(move || {
                    counts.frees[slot].fetch_add(1, Ordering::Relaxed);
                })
            })
            .collect();
        let observed = counts.frees();
        assert!([usize::MAX, 0, 1].contains(&observed));
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(counts.frees(), 1);
    });
}

#[test]
fn per_slot_overflow_preserves_modulo_events() {
    model_overflow([usize::MAX, 0]);
}

#[test]
fn folded_sum_overflow_preserves_modulo_events() {
    model_overflow([usize::MAX - 1, 1]);
}

#[test]
fn publication_and_drain_keep_original_gauges_exact() {
    loom::model(|| {
        let counts = Arc::new(Counts::new([0, 0]));
        let head = Arc::new(AtomicUsize::new(0));
        let payload = Arc::new(AtomicUsize::new(0));
        let writer_counts = Arc::clone(&counts);
        let writer_head = Arc::clone(&head);
        let writer_payload = Arc::clone(&payload);
        let writer = thread::spawn(move || {
            writer_counts.progress.fetch_add(1, Ordering::Relaxed);
            writer_counts.frees[0].fetch_add(1, Ordering::Relaxed);
            writer_counts.pending.fetch_add(1, Ordering::Relaxed);
            writer_payload.store(42, Ordering::Relaxed);
            writer_head.compare_exchange(0, 1, Ordering::Release, Ordering::Relaxed).unwrap();
            assert_ne!(writer_counts.progress.fetch_sub(1, Ordering::Relaxed), 0);
        });
        if head.swap(0, Ordering::Acquire) == 1 {
            assert_eq!(payload.load(Ordering::Relaxed), 42);
            counts.drain(1);
        }
        writer.join().unwrap();
        if head.swap(0, Ordering::Acquire) == 1 {
            assert_eq!(payload.load(Ordering::Relaxed), 42);
            counts.drain(1);
        }
        assert_eq!(
            (
                counts.frees(),
                counts.drained[1].load(Ordering::Relaxed),
                counts.pending.load(Ordering::Relaxed),
                counts.progress.load(Ordering::Relaxed)
            ),
            (1, 1, 0, 0)
        );
    });
}

#[test]
#[should_panic(expected = "colliding load/store lost events")]
fn rejected_owner_only_updates_lose_colliding_events() {
    loom::model(|| {
        let counts = Arc::new(Counts::new([0, 0]));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let counts = Arc::clone(&counts);
                thread::spawn(move || {
                    let old = counts.frees[0].load(Ordering::Relaxed);
                    counts.frees[0].store(old + 1, Ordering::Relaxed);
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(counts.frees(), 2, "colliding load/store lost events");
    });
}

#[test]
#[should_panic(expected = "naive pending fold reports an impossible gauge")]
fn rejected_gauge_fold_can_report_a_value_that_never_existed() {
    loom::model(|| {
        let gauges = Arc::new([AtomicUsize::new(1), AtomicUsize::new(0)]);
        let first = gauges[0].load(Ordering::SeqCst);
        let writer_gauges = Arc::clone(&gauges);
        thread::spawn(move || {
            writer_gauges[0].fetch_sub(1, Ordering::SeqCst);
            writer_gauges[1].fetch_add(1, Ordering::SeqCst);
        })
        .join()
        .unwrap();
        let reported = first + gauges[1].load(Ordering::SeqCst);
        assert!(reported <= 1, "naive pending fold reports an impossible gauge");
    });
}
