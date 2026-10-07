// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Safe task wakers outlive pinned task storage after success, panic, or shutdown.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

mod support;

use std::cell::Cell;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};

use panic_support::runtime;
use support::JoinHandleExt as _;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

testing_aids::init_tracing!();

#[derive(Clone, Copy, Debug)]
enum Finish {
    Success,
    Panic,
    Pending,
}

struct PinnedTask {
    finish: Finish,
    sent: Cell<Option<mpsc::Sender<Waker>>>,
    drops: Arc<AtomicUsize>,
    address: Cell<Option<usize>>,
    owner: std::thread::ThreadId,
    _local: Rc<()>,
    _pinned: PhantomPinned,
}

impl PinnedTask {
    fn new(finish: Finish, sent: mpsc::Sender<Waker>, drops: Arc<AtomicUsize>) -> Self {
        Self {
            finish,
            sent: Cell::new(Some(sent)),
            drops,
            address: Cell::new(None),
            owner: std::thread::current().id(),
            _local: Rc::new(()),
            _pinned: PhantomPinned,
        }
    }
}

impl Future for PinnedTask {
    type Output = u32;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
        let this = self.as_ref().get_ref();
        let address = std::ptr::from_ref(this).addr();
        if let Some(previous) = this.address.replace(Some(address)) {
            assert_eq!(previous, address);
        }
        if let Some(sent) = this.sent.take() {
            sent.send(cx.waker().clone()).unwrap();
        }
        match this.finish {
            Finish::Success => Poll::Ready(42),
            Finish::Panic => panic!("pinned task poll panic"),
            Finish::Pending => Poll::Pending,
        }
    }
}

impl Drop for PinnedTask {
    fn drop(&mut self) {
        assert_eq!(Some(std::ptr::from_ref(self).addr()), self.address.get());
        assert_eq!(std::thread::current().id(), self.owner);
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

fn wake_after_retirement(waker: Waker) {
    std::thread::spawn(move || {
        for _ in 0..8 {
            let clone = waker.clone();
            clone.wake_by_ref();
            clone.wake();
        }
        waker.wake();
    })
    .join()
    .unwrap();
}

#[test]
fn remote_retained_wakers_after_success() {
    remote_case(Finish::Success);
}

#[test]
fn remote_retained_wakers_after_panic() {
    remote_case(Finish::Panic);
}

#[test]
fn remote_retained_wakers_after_cancellation() {
    remote_case(Finish::Pending);
}

fn remote_case(finish: Finish) {
    eprintln!("remote {finish:?}: constructing runtime");
    let runtime = runtime();
    let (sent, received) = mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware((finish, sent, Arc::clone(&drops))), |_, Unaware((finish, sent, drops))| {
            PinnedTask::new(finish, sent, drops)
        });
    let retained = received.recv_timeout(TEST_TIMEOUT).unwrap();
    eprintln!("remote {finish:?}: retained waker received");
    let outcome = match finish {
        Finish::Pending => {
            eprintln!("remote {finish:?}: stopping runtime");
            runtime.stop().unwrap();
            eprintln!("remote {finish:?}: stopped");
            task.join()
        }
        Finish::Success | Finish::Panic => {
            let outcome = task.join();
            eprintln!("remote {finish:?}: joined");
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            retained.wake_by_ref();
            assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 7 }).join().unwrap(), 7);
            eprintln!("remote {finish:?}: sentinel joined; stopping runtime");
            runtime.stop().unwrap();
            eprintln!("remote {finish:?}: stopped");
            outcome
        }
    };
    match finish {
        Finish::Success => assert_eq!(outcome.unwrap(), 42),
        Finish::Panic => assert!(outcome.unwrap_err().is_panic()),
        Finish::Pending => assert!(outcome.unwrap_err().is_shutdown()),
    }
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    wake_after_retirement(retained);
}

#[test]
fn local_retained_wakers_after_success() {
    local_case(Finish::Success);
}

#[test]
fn local_retained_wakers_after_panic() {
    local_case(Finish::Panic);
}

#[test]
fn local_retained_wakers_after_cancellation() {
    local_case(Finish::Pending);
}

fn local_case(finish: Finish) {
    eprintln!("local {finish:?}: constructing runtime");
    let runtime = runtime();
    let (sent, received) = mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&drops);
    runtime
        .scheduler()
        .block_on(async move |cx| {
            let task = cx.local_scheduler().unwrap().spawn(move || PinnedTask::new(finish, sent, captured));
            match finish {
                Finish::Success => assert_eq!(task.await.unwrap(), 42),
                Finish::Panic => assert!(task.await.unwrap_err().is_panic()),
                Finish::Pending => {
                    drop(task);
                    cx.scheduler().spawn(async |_| {}).await.unwrap();
                }
            }
        })
        .unwrap();
    eprintln!("local {finish:?}: root joined");
    let retained = received.recv_timeout(TEST_TIMEOUT).unwrap();
    if matches!(finish, Finish::Success | Finish::Panic) {
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
    eprintln!("local {finish:?}: retained waker received; stopping runtime");
    runtime.stop().unwrap();
    eprintln!("local {finish:?}: stopped");
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    wake_after_retirement(retained);
}

#[test]
fn completion_racing_stop_destroys_each_future_once() {
    let repetitions = if cfg!(miri) { 2 } else { 32 };
    for _ in 0..repetitions {
        struct RacingTask {
            started: Option<mpsc::Sender<Waker>>,
            finish: Arc<AtomicBool>,
            drops: Arc<AtomicUsize>,
        }

        impl Future for RacingTask {
            type Output = u32;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
                if let Some(started) = self.started.take() {
                    started.send(cx.waker().clone()).unwrap();
                }
                if self.finish.load(Ordering::SeqCst) {
                    Poll::Ready(42)
                } else {
                    Poll::Pending
                }
            }
        }

        impl Drop for RacingTask {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }

        let runtime = runtime();
        let operations = arty::runtime::RuntimeOperations::from(&runtime);
        let (started, received) = mpsc::channel();
        let finish = Arc::new(AtomicBool::new(false));
        let drops = Arc::new(AtomicUsize::new(0));
        let task = runtime.scheduler().spawn_anywhere(
            Unaware((started, Arc::clone(&finish), Arc::clone(&drops))),
            |_, Unaware((started, finish, drops))| RacingTask {
                started: Some(started),
                finish,
                drops,
            },
        );
        let waker = received.recv_timeout(TEST_TIMEOUT).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let stop_barrier = Arc::clone(&barrier);
        let stopper = std::thread::spawn(move || {
            stop_barrier.wait();
            for _ in 0..8 {
                operations.request_stop();
            }
        });
        let finish_barrier = Arc::clone(&barrier);
        let completer = std::thread::spawn(move || {
            finish_barrier.wait();
            finish.store(true, Ordering::SeqCst);
            waker.wake();
        });
        barrier.wait();
        stopper.join().unwrap();
        completer.join().unwrap();
        runtime.stop().unwrap();
        match task.join() {
            Ok(value) => assert_eq!(value, 42),
            Err(error) => assert!(error.is_shutdown()),
        }
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
