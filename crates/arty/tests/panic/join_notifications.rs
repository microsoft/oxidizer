// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A safe receiver waker may panic while a task publishes its outcome.

use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use arty::runtime::{BlockingPoolPolicy, PanicPolicy, Runtime, WorkersPolicy};
use observed_testing::{TEST_ID, test_emitter};
use thread_aware::Unaware;

use crate::runtime_support::runtime;
use crate::support::{self, JoinHandleExt as _};

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
    assert_remote_notification_preserves_outcome(false);
}

#[test]
fn remote_panicking_join_waker_preserves_published_panic() {
    assert_remote_notification_preserves_outcome(true);
}

fn assert_remote_notification_preserves_outcome(panics: bool) {
    let (sink, processor) = test_emitter(TEST_ID);
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .panic(PanicPolicy::ignore())
        .sink(sink)
        .build()
        .unwrap();
    let (release, gate) = events_once::Event::boxed();
    let join = runtime
        .scheduler()
        .spawn_anywhere(Unaware((gate, panics)), |_, Unaware((gate, panics))| async move {
            gate.await.unwrap();
            if panics {
                panic!("remote task panic");
            }
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
    // The flag records notification only. The result channel and captured
    // events establish the task outcome independently.
    let outcome = futures::executor::block_on(join);
    if panics {
        let error = outcome.unwrap_err();
        assert!(error.is_panic());
        assert_eq!(*error.into_panic().unwrap().downcast::<&str>().unwrap(), "remote task panic");
    } else {
        assert_eq!(outcome.unwrap(), 42);
    }
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 7 }).join().unwrap(), 7);
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    runtime.stop().unwrap();
    let events = processor.events();
    assert_eq!(
        events.iter().filter(|event| event.name() == "arty.rt.task.succeeded").count(),
        if panics { 1 } else { 2 }
    );
    assert_eq!(
        events.iter().filter(|event| event.name() == "arty.rt.task.panicked").count(),
        usize::from(panics)
    );
}

#[test]
fn remote_panicking_join_waker_on_cancellation_is_contained() {
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

    support::runtime_control(&runtime).request_stop();
    runtime.stop().unwrap();

    assert!(join.as_mut().poll(&mut Context::from_waker(&waker)).is_ready());
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    assert!(notified.load(Ordering::SeqCst));
}

#[test]
fn blocking_panicking_join_waker_is_contained() {
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
}

#[test]
fn blocking_panicking_join_waker_on_cancellation_is_contained() {
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

    support::runtime_control(&runtime).request_stop();
    release_tx.send(()).unwrap();
    running.join().unwrap();
    runtime.stop().unwrap();

    let Poll::Ready(Err(error)) = queued.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
        panic!("queued blocking work must report shutdown");
    };
    assert!(error.is_shutdown());
    assert!(notified.load(Ordering::SeqCst));
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
}
