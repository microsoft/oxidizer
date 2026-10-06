// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A safe receiver waker may panic while a task publishes its successful result.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::future::poll_fn;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use arty::runtime::RuntimeOperations;
use panic_support::{isolated, runtime};
use thread_aware::Unaware;

testing_aids::init_tracing!();

struct PanicWake {
    notified: Arc<AtomicBool>,
    notifications: Arc<AtomicUsize>,
}

impl Wake for PanicWake {
    fn wake(self: Arc<Self>) {
        self.notifications.fetch_add(1, Ordering::SeqCst);
        self.notified.store(true, Ordering::SeqCst);
        assert!(!std::thread::panicking());
        panic!("join receiver notification");
    }
}

#[test]
fn remote_panicking_join_waker_preserves_published_result() {
    isolated("remote_panicking_join_waker_preserves_published_result", || {
        let runtime = runtime();
        let (release, gate) = events_once::Event::boxed();
        let join = runtime.scheduler().spawn_anywhere(Unaware(gate), |_, Unaware(gate)| async move {
            gate.await.unwrap();
            42
        });
        let mut join = pin!(join);
        let notified = Arc::new(AtomicBool::new(false));
        let notifications = Arc::new(AtomicUsize::new(0));
        let waker = Waker::from(Arc::new(PanicWake {
            notified: Arc::clone(&notified),
            notifications: Arc::clone(&notifications),
        }));
        assert!(join.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        release.send(());
        let deadline = std::time::Instant::now() + testing_aids::TEST_TIMEOUT;
        while !notified.load(Ordering::SeqCst) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(futures::executor::block_on(join).unwrap(), 42);
        assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 7 }).wait().unwrap(), 7);
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        runtime.stop().unwrap();
    });
}

#[test]
fn local_panicking_join_waker_preserves_published_result() {
    isolated("local_panicking_join_waker_preserves_published_result", || {
        let runtime = runtime();
        runtime
            .scheduler()
            .block_on(async |cx| {
                let (release, gate) = events_once::Event::boxed();
                let join = cx.local_scheduler().unwrap().spawn(async move || {
                    gate.await.unwrap();
                    std::rc::Rc::new(42)
                });
                let mut join = pin!(join);
                let notified = Arc::new(AtomicBool::new(false));
                let notifications = Arc::new(AtomicUsize::new(0));
                let waker = Waker::from(Arc::new(PanicWake {
                    notified: Arc::clone(&notified),
                    notifications: Arc::clone(&notifications),
                }));
                assert!(join.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
                release.send(());
                poll_fn(|context| {
                    if notified.load(Ordering::SeqCst) {
                        Poll::Ready(())
                    } else {
                        context.waker().wake_by_ref();
                        Poll::Pending
                    }
                })
                .await;
                assert_eq!(*join.await.unwrap(), 42);
                assert_eq!(notifications.load(Ordering::SeqCst), 1);
                assert_eq!(*cx.local_scheduler().unwrap().spawn(async || std::rc::Rc::new(7)).await.unwrap(), 7);
            })
            .unwrap();
        runtime.stop().unwrap();
    });
}

#[test]
fn remote_panicking_join_waker_on_cancellation_is_contained() {
    isolated("remote_panicking_join_waker_on_cancellation_is_contained", || {
        let runtime = runtime();
        let join = runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| async { std::future::pending::<()>().await });
        let mut join = pin!(join);
        let notified = Arc::new(AtomicBool::new(false));
        let notifications = Arc::new(AtomicUsize::new(0));
        let waker = Waker::from(Arc::new(PanicWake {
            notified: Arc::clone(&notified),
            notifications: Arc::clone(&notifications),
        }));
        assert!(join.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

        RuntimeOperations::from(&runtime).request_stop();
        runtime.stop().unwrap();

        assert!(join.as_mut().poll(&mut Context::from_waker(&waker)).is_ready());
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        assert!(notified.load(Ordering::SeqCst));
    });
}

#[test]
fn blocking_panicking_join_waker_is_contained() {
    isolated("blocking_panicking_join_waker_is_contained", || {
        let runtime = runtime();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let join = runtime.scheduler().spawn_blocking(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            42
        });
        let mut join = pin!(join);
        let notified = Arc::new(AtomicBool::new(false));
        let notifications = Arc::new(AtomicUsize::new(0));
        let waker = Waker::from(Arc::new(PanicWake {
            notified: Arc::clone(&notified),
            notifications: Arc::clone(&notifications),
        }));
        assert!(join.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        started_rx.recv().unwrap();
        release_tx.send(()).unwrap();

        let deadline = std::time::Instant::now() + testing_aids::TEST_TIMEOUT;
        while !notified.load(Ordering::SeqCst) {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(futures::executor::block_on(join).unwrap(), 42);
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        runtime.stop().unwrap();
    });
}

#[test]
fn blocking_panicking_join_waker_on_cancellation_is_contained() {
    isolated("blocking_panicking_join_waker_on_cancellation_is_contained", || {
        let runtime = runtime();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let running = runtime.scheduler().spawn_blocking(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();

        let queued = runtime.scheduler().spawn_blocking(|| 42);
        let mut queued = pin!(queued);
        let notified = Arc::new(AtomicBool::new(false));
        let notifications = Arc::new(AtomicUsize::new(0));
        let waker = Waker::from(Arc::new(PanicWake {
            notified: Arc::clone(&notified),
            notifications: Arc::clone(&notifications),
        }));
        assert!(queued.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());

        RuntimeOperations::from(&runtime).request_stop();
        release_tx.send(()).unwrap();
        running.wait().unwrap();
        runtime.stop().unwrap();

        let Poll::Ready(Err(error)) = queued.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
            panic!("queued blocking work must report shutdown");
        };
        assert!(error.is_shutdown());
        assert!(notified.load(Ordering::SeqCst));
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
    });
}
