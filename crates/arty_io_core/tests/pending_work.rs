// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime-backed completion ownership and synchronous registration.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Wake, Waker};
use std::thread;
use std::time::{Duration, Instant};

use arty_io_core::{Cycle, PendingWork, PendingWorkTracker};
use static_assertions::{assert_impl_all, assert_not_impl_any};

assert_impl_all!(PendingWork: Send, Sync, std::fmt::Debug);
assert_not_impl_any!(PendingWork: Clone, Copy);
assert_not_impl_any!(Cycle<'static>: Send, Sync);

#[derive(Default)]
struct Counter(AtomicUsize);

impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn work() -> (PendingWork, Arc<Counter>) {
    let completed = Arc::new(Counter::default());
    let work = PendingWork::new(Waker::from(Arc::clone(&completed)));
    (work, completed)
}

#[test]
fn construction_keeps_the_participation_outstanding() {
    let (work, completed) = work();

    assert_eq!(completed.0.load(Ordering::Relaxed), 0);
    assert_eq!(format!("{work:?}"), "PendingWork { .. }");
    drop(work);
}

#[test]
fn completion_notifies_once() {
    let (work, completed) = work();

    work.complete();

    assert_eq!(completed.0.load(Ordering::Relaxed), 1);
}

#[test]
fn dropping_notifies_once() {
    let (work, completed) = work();

    drop(work);

    assert_eq!(completed.0.load(Ordering::Relaxed), 1);
}

#[test]
fn unrelated_waker_ownership_does_not_complete_work() {
    let completed = Arc::new(Counter::default());
    let on_complete = Waker::from(Arc::clone(&completed));
    let work = PendingWork::new(on_complete.clone());

    drop((on_complete.clone(), on_complete));
    assert_eq!(completed.0.load(Ordering::Relaxed), 0);

    drop(work);
    assert_eq!(completed.0.load(Ordering::Relaxed), 1);
}

#[test]
fn background_work_can_complete_or_drop_its_participation() {
    for explicit in [false, true] {
        let (work, completed) = work();
        thread::spawn(move || {
            if explicit {
                work.complete();
            } else {
                drop(work);
            }
        })
        .join()
        .unwrap();

        assert_eq!(completed.0.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn unwinding_completes_pending_work() {
    let (work, completed) = work();

    let result = catch_unwind(AssertUnwindSafe(move || {
        let _work = work;
        panic!("background work failed");
    }));

    assert!(result.is_err());
    assert_eq!(completed.0.load(Ordering::Relaxed), 1);
}

#[test]
fn panicking_notification_is_not_repeated() {
    struct PanickingWake(AtomicUsize);

    impl Wake for PanickingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
            panic!("runtime completion callback failed");
        }
    }

    for explicit in [false, true] {
        let count = Arc::new(PanickingWake(AtomicUsize::new(0)));
        let work = PendingWork::new(Waker::from(Arc::clone(&count)));
        let result = catch_unwind(AssertUnwindSafe(|| {
            if explicit {
                work.complete();
            } else {
                drop(work);
            }
        }));

        assert!(result.is_err());
        assert_eq!(count.0.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn cycle_supports_a_tracker_with_borrowed_worker_local_state() {
    struct LocalTracker<'a> {
        registrations: &'a Cell<usize>,
        completed: Waker,
    }

    impl PendingWorkTracker for LocalTracker<'_> {
        fn start_work(&mut self, interrupt: Waker) -> PendingWork {
            self.registrations.update(|count| count + 1);
            interrupt.wake();
            PendingWork::new(self.completed.clone())
        }
    }

    assert_not_impl_any!(LocalTracker<'static>: Send, Sync);

    let interrupted = Arc::new(Counter::default());
    let completed = Arc::new(Counter::default());
    let registrations = Cell::new(0);
    let mut tracker = LocalTracker {
        registrations: &registrations,
        completed: Waker::from(Arc::clone(&completed)),
    };
    let mut cycle = Cycle::new(Instant::now(), Duration::ZERO, &mut tracker);

    let first = cycle.start_work(Waker::from(Arc::clone(&interrupted)));
    assert_eq!(interrupted.0.load(Ordering::Relaxed), 1);
    let second = cycle.start_work(Waker::from(Arc::clone(&interrupted)));
    assert_eq!(interrupted.0.load(Ordering::Relaxed), 2);
    assert_eq!(completed.0.load(Ordering::Relaxed), 0);

    first.complete();
    assert_eq!(completed.0.load(Ordering::Relaxed), 1);
    drop(second);

    assert_eq!(registrations.get(), 2);
    assert_eq!(completed.0.load(Ordering::Relaxed), 2);
}

#[test]
fn cycle_preserves_its_runtime_inputs_without_registering_work() {
    struct UnexpectedWork;

    impl PendingWorkTracker for UnexpectedWork {
        fn start_work(&mut self, _interrupt: Waker) -> PendingWork {
            panic!("reading cycle inputs must not register work");
        }
    }

    let started_at = Instant::now();
    let mut tracker = UnexpectedWork;
    for max_wait in [Duration::ZERO, Duration::from_millis(17)] {
        let cycle = Cycle::new(started_at, max_wait, &mut tracker);

        assert_eq!(cycle.started_at(), started_at);
        assert_eq!(cycle.max_wait(), max_wait);
        assert_eq!(
            format!("{cycle:?}"),
            format!("Cycle {{ started_at: {started_at:?}, max_wait: {max_wait:?}, .. }}")
        );
    }
}
