// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A timer installed during cancellation runs only after task retirement.

#![cfg(not(miri))] // This contract exercises the native timer driver and process boundary.

use std::error::Error as _;
use std::future::pending;
use std::pin::Pin;
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use arty::time::Clock;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

use crate::runtime_support::runtime;
use crate::support::JoinHandleExt as _;

struct PanicWake;

impl Wake for PanicWake {
    fn wake(self: Arc<Self>) {
        panic!("final shutdown timer callback");
    }
}

struct WorkerPanicWake(mpsc::Sender<()>);

impl Wake for WorkerPanicWake {
    fn wake(self: Arc<Self>) {
        self.0.send(()).unwrap();
        panic!("worker timer callback");
    }
}

struct CancellationTimer {
    clock: Clock,
    started: Option<mpsc::Sender<Waker>>,
    dropped: mpsc::Sender<()>,
}

impl Future for CancellationTimer {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if let Some(started) = self.started.take() {
            started.send(cx.waker().clone()).unwrap();
        }
        Poll::Pending
    }
}

impl Drop for CancellationTimer {
    fn drop(&mut self) {
        let duration = Duration::from_millis(2);
        let mut timer = self.clock.delay(duration);
        let waker = Waker::from(Arc::new(PanicWake));
        assert!(Pin::new(&mut timer).poll(&mut Context::from_waker(&waker)).is_pending());
        // Keep the registration alive; a poisoned timer lock cannot be used by Delay::drop.
        std::mem::forget(timer);
        let watch = self.clock.stopwatch();
        let deadline = Instant::now() + TEST_TIMEOUT;
        while watch.elapsed() < duration {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        self.dropped.send(()).unwrap();
    }
}

#[test]
fn final_timer_panic_reports_worker_failure_after_joining_blocking_work() {
    timer_case(true);
}

#[test]
fn final_timer_without_escaped_task_waker_reports_joined_worker_failure() {
    timer_case(false);
}

#[test]
fn worker_panic_cause_reaches_other_task() {
    testing_aids::execute_or_abandon(|| {
        let runtime = runtime();
        let (started, ready) = mpsc::channel();
        let task = runtime
            .scheduler()
            .spawn_anywhere(Unaware(started), |_, Unaware(started)| async move {
                started.send(()).unwrap();
                pending::<()>().await;
            });
        ready.recv_timeout(TEST_TIMEOUT).unwrap();
        let (panicked, panic_observed) = mpsc::channel();
        let timer = runtime.scheduler().spawn_anywhere(Unaware(panicked), |cx, Unaware(panicked)| async move {
            let mut timer = cx.clock().delay(Duration::from_millis(2));
            let waker = Waker::from(Arc::new(WorkerPanicWake(panicked)));
            assert!(Pin::new(&mut timer).poll(&mut Context::from_waker(&waker)).is_pending());
            std::mem::forget(timer);
        });
        timer.join().unwrap();
        panic_observed.recv_timeout(TEST_TIMEOUT).unwrap();

        let error = task.join().unwrap_err();

        assert!(error.is_shutdown());
        assert_eq!(error.source().unwrap().to_string(), "a runtime worker panicked");
        assert_eq!(runtime.stop().unwrap_err().to_string(), "a runtime worker panicked");
    })
    .unwrap();
}

fn timer_case(retain_waker: bool) {
    let runtime = runtime();
    let (started, ready) = mpsc::channel();
    let (dropped, retired) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware((started, dropped)), |cx, Unaware((started, dropped))| CancellationTimer {
            clock: cx.clock().clone(),
            started: Some(started),
            dropped,
        });
    let retained = ready.recv_timeout(TEST_TIMEOUT).unwrap();
    let retained = if retain_waker {
        Some(retained)
    } else {
        drop(retained);
        None
    };
    let (blocking_started, blocking_ready) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let (finished, completed) = mpsc::channel();
    let blocking = runtime.scheduler().spawn_blocking(move || {
        blocking_started.send(()).unwrap();
        gate.recv_timeout(TEST_TIMEOUT).unwrap();
        finished.send(()).unwrap();
        42
    });
    blocking_ready.recv_timeout(TEST_TIMEOUT).unwrap();
    let stopper = std::thread::spawn(move || runtime.stop());
    retired.recv_timeout(TEST_TIMEOUT).unwrap();
    release.send(()).unwrap();
    assert!(stopper.join().unwrap().is_err());
    completed.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(blocking.join().unwrap(), 42);
    assert!(task.join().unwrap_err().is_shutdown());
    if let Some(retained) = retained {
        retained.wake_by_ref();
        let consuming = retained.clone();
        consuming.wake();
        drop(retained);
    }
}
