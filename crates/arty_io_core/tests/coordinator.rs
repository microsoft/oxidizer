// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Coordinator interruption, token ownership, and cycle-transition races.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Wake, Waker};
use std::thread;
use std::time::{Duration, Instant};

use arty_io_core::{Coordinator, Cycle, PendingWork};
use static_assertions::{assert_impl_all, assert_not_impl_any};

assert_impl_all!(Coordinator: Send, Sync, std::fmt::Debug);
assert_not_impl_any!(Coordinator: Clone);
assert_impl_all!(PendingWork: Send, Sync, std::fmt::Debug);
assert_not_impl_any!(PendingWork: Clone);

#[derive(Default)]
struct Counter(AtomicUsize);

impl Wake for Counter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn interrupt_is_one_shot_and_late_registration_is_not_lost() {
    let coordinator = Coordinator::new();
    let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
    let token = cycle.start_work();
    let first = Arc::new(Counter::default());
    let late = Arc::new(Counter::default());
    token.on_interrupt(Waker::from(Arc::clone(&first)));

    coordinator.interrupt();
    coordinator.interrupt();
    token.on_interrupt(Waker::from(Arc::clone(&late)));

    assert_eq!(first.0.load(Ordering::Relaxed), 1);
    assert_eq!(late.0.load(Ordering::Relaxed), 1);
    assert!(token.is_interrupted());
    drop(token);
    coordinator.complete_cycle();
}

#[test]
fn outstanding_work_blocks_cycle_completion_until_the_token_is_dropped() {
    let mut coordinator = Coordinator::new();
    coordinator.begin_cycle();
    let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
    let token = cycle.start_work();
    let (release_tx, release_rx) = mpsc::channel();
    let (waiting_tx, waiting_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();

    thread::scope(|scope| {
        scope.spawn(move || {
            release_rx.recv().unwrap();
            drop(token);
        });
        scope.spawn(|| {
            waiting_tx.send(()).unwrap();
            coordinator.complete_cycle();
            done_tx.send(()).unwrap();
        });
        waiting_rx.recv().unwrap();
        assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    });
}

#[test]
fn dropping_multiple_tokens_releases_the_completion_barrier() {
    let mut coordinator = Coordinator::new();
    coordinator.begin_cycle();
    {
        let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
        let first = cycle.start_work();
        let second = cycle.start_work();
        drop(first);
        drop(second);
    }
    coordinator.complete_cycle();
    coordinator.begin_cycle();
}

#[test]
fn complete_interrupts_waiters_and_releases_the_barrier() {
    let mut coordinator = Coordinator::new();
    coordinator.begin_cycle();
    let count = Arc::new(Counter::default());
    let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
    let token = cycle.start_work();
    token.on_interrupt(Waker::from(Arc::clone(&count)));
    token.complete();

    coordinator.complete_cycle();
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
}

#[test]
fn old_broadcast_finishing_after_cycle_transition_preserves_new_registrations() {
    struct HeldWake {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl Wake for HeldWake {
        fn wake(self: Arc<Self>) {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv_timeout(Duration::from_secs(10)).unwrap();
        }
    }

    let mut coordinator = Coordinator::new();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (stable_waker, thread) = {
        let cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
        let token = cycle.start_work();
        let stable_waker = coordinator.interrupt_waker();
        token.on_interrupt(Waker::from(Arc::new(HeldWake {
            entered: entered_tx,
            release: Mutex::new(release_rx),
        })));

        let broadcaster = stable_waker.clone();
        let thread = thread::spawn(move || broadcaster.wake());
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(token);
        (stable_waker, thread)
    };
    coordinator.begin_cycle();
    let next_cycle = Cycle::new(Instant::now(), Duration::ZERO, &coordinator);
    let next = Arc::new(Counter::default());
    let next_token = next_cycle.start_work();
    next_token.on_interrupt(Waker::from(Arc::clone(&next)));
    release_tx.send(()).unwrap();
    thread.join().unwrap();

    stable_waker.wake_by_ref();
    assert_eq!(next.0.load(Ordering::Relaxed), 1);
    drop(next_token);
    coordinator.complete_cycle();
}
