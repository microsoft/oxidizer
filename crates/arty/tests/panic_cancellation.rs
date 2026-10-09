// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shutdown must finish task destruction despite a sole cancellation Drop panic.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

mod support;

use std::cell::Cell;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll};

use arty::runtime::RuntimeOperations;
use panic_support::runtime;
use support::JoinHandleExt as _;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

testing_aids::init_tracing!();

struct PendingDropPanic {
    started: Cell<Option<mpsc::Sender<()>>>,
    dropped: mpsc::Sender<()>,
    drops: Arc<AtomicUsize>,
    address: Cell<Option<usize>>,
    owner: std::thread::ThreadId,
    _local: Rc<()>,
    _pinned: PhantomPinned,
}

impl Future for PendingDropPanic {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        let this = self.as_ref().get_ref();
        this.address.set(Some(std::ptr::from_ref(this).addr()));
        if let Some(started) = this.started.take() {
            started.send(()).unwrap();
        }
        Poll::Pending
    }
}

impl Drop for PendingDropPanic {
    fn drop(&mut self) {
        assert_eq!(Some(std::ptr::from_ref(self).addr()), self.address.get());
        assert_eq!(std::thread::current().id(), self.owner);
        self.drops.fetch_add(1, Ordering::SeqCst);
        self.dropped.send(()).unwrap();
        assert!(!std::thread::panicking());
        panic!("cancelled future destructor");
    }
}

fn pending_drop(started: mpsc::Sender<()>, dropped: mpsc::Sender<()>, drops: Arc<AtomicUsize>) -> PendingDropPanic {
    PendingDropPanic {
        started: Cell::new(Some(started)),
        dropped,
        drops,
        address: Cell::new(None),
        owner: std::thread::current().id(),
        _local: Rc::new(()),
        _pinned: PhantomPinned,
    }
}

#[test]
fn remote_cancellation_drop_panic_completes_shutdown() {
    let runtime = runtime();
    let (started, ready) = mpsc::channel();
    let (dropped, received) = mpsc::channel();
    let drops = Arc::new(AtomicUsize::new(0));
    let task = runtime.scheduler().spawn_anywhere(
        Unaware((started, dropped, Arc::clone(&drops))),
        |_, Unaware((started, dropped, drops))| pending_drop(started, dropped, drops),
    );
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    runtime.stop().unwrap();
    received.recv_timeout(TEST_TIMEOUT).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(task.join().unwrap_err().is_shutdown());
}

#[test]
fn borrowing_cancellation_drop_panic_releases_caller_storage() {
    use std::error::Error as _;

    struct Borrowed<'a>(&'a AtomicUsize);

    impl Drop for Borrowed<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
            assert!(!std::thread::panicking());
            panic!("borrowed cancellation destructor");
        }
    }

    let runtime = runtime();
    let drops = AtomicUsize::new(0);
    let borrowed = Borrowed(&drops);
    let error = runtime
        .scheduler()
        .block_on(async move |cx| {
            let guard = borrowed;
            RuntimeOperations::from(&cx).request_stop();
            std::future::pending::<()>().await;
            drop(guard);
        })
        .unwrap_err();
    assert!(
        error
            .source()
            .unwrap()
            .downcast_ref::<arty::task::JoinError>()
            .unwrap()
            .is_shutdown()
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    runtime.stop().unwrap();
}
