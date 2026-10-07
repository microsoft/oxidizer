// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A user poll panic racing shutdown cannot lose its join or destroy a task twice.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::cell::Cell;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::task::{Context, Poll, Waker};

use arty::runtime::RuntimeOperations;
use panic_support::runtime;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

testing_aids::init_tracing!();

struct RacingPanic {
    first: Cell<Option<mpsc::Sender<Waker>>>,
    ready: Arc<AtomicBool>,
    drops: Arc<AtomicUsize>,
    address: Cell<Option<usize>>,
    owner: std::thread::ThreadId,
    _local: Rc<()>,
    _pinned: PhantomPinned,
}

impl Future for RacingPanic {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let address = std::ptr::from_ref(self.as_ref().get_ref()).addr();
        if let Some(previous) = self.address.replace(Some(address)) {
            assert_eq!(previous, address);
        }
        if let Some(first) = self.first.take() {
            first.send(cx.waker().clone()).unwrap();
        }
        assert!(!self.ready.load(Ordering::SeqCst), "task poll racing stop");
        Poll::Pending
    }
}

impl Drop for RacingPanic {
    fn drop(&mut self) {
        assert_eq!(Some(std::ptr::from_ref(self).addr()), self.address.get());
        assert_eq!(std::thread::current().id(), self.owner);
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn poll_panic_racing_repeated_stop_always_completes_one_join_and_drop() {
    let repetitions = if cfg!(miri) { 2 } else { 24 };
    for _ in 0..repetitions {
        let runtime = runtime();
        let operations = RuntimeOperations::from(&runtime);
        let (first, received) = mpsc::channel();
        let ready = Arc::new(AtomicBool::new(false));
        let drops = Arc::new(AtomicUsize::new(0));
        let task = runtime.scheduler().spawn_anywhere(
            Unaware((first, Arc::clone(&ready), Arc::clone(&drops))),
            |_, Unaware((first, ready, drops))| RacingPanic {
                first: Cell::new(Some(first)),
                ready,
                drops,
                address: Cell::new(None),
                owner: std::thread::current().id(),
                _local: Rc::new(()),
                _pinned: PhantomPinned,
            },
        );
        let retained = received.recv_timeout(TEST_TIMEOUT).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let stop_barrier = Arc::clone(&barrier);
        let stopper = std::thread::spawn(move || {
            stop_barrier.wait();
            for _ in 0..8 {
                operations.request_stop();
            }
        });
        let panic_barrier = Arc::clone(&barrier);
        let panicker = std::thread::spawn(move || {
            panic_barrier.wait();
            ready.store(true, Ordering::SeqCst);
            retained.wake();
        });
        barrier.wait();
        stopper.join().unwrap();
        panicker.join().unwrap();
        runtime.stop().unwrap();
        let error = task.wait().unwrap_err();
        assert!(error.is_panic() || error.is_shutdown());
        assert_ne!(error.is_panic(), error.is_shutdown());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
