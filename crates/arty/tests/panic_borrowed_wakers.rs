// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Completed scoped-task wakers cannot retain caller borrows after the root returns.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::cell::Cell;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::task::{Context, Poll, Waker};

use panic_support::{isolated, runtime};
use testing_aids::TEST_TIMEOUT;

testing_aids::init_tracing!();

struct Borrowed<'a> {
    text: &'a str,
    drops: &'a AtomicUsize,
    sent: Cell<Option<mpsc::Sender<Waker>>>,
    address: Cell<Option<usize>>,
    panic_poll: bool,
    _local: Rc<()>,
    _pinned: PhantomPinned,
}

impl Future for Borrowed<'_> {
    type Output = usize;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<usize> {
        self.address.set(Some(std::ptr::from_ref(self.as_ref().get_ref()).addr()));
        if let Some(sent) = self.sent.take() {
            sent.send(cx.waker().clone()).unwrap();
        }
        assert!(!self.panic_poll, "borrowed root poll");
        Poll::Ready(self.text.len())
    }
}

impl Drop for Borrowed<'_> {
    fn drop(&mut self) {
        assert_eq!(Some(std::ptr::from_ref(self).addr()), self.address.get());
        assert_eq!(self.text, "caller-owned storage");
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn scoped_wakers_remain_valid_after_caller_storage_is_destroyed() {
    isolated("scoped_wakers_remain_valid_after_caller_storage_is_destroyed", || {
        for panic_poll in [false, true] {
            let runtime = runtime();
            let retained = {
                let text = String::from("caller-owned storage");
                let drops = AtomicUsize::new(0);
                let (sent, received) = mpsc::channel();
                let result = runtime.scheduler().block_on(|_| Borrowed {
                    text: &text,
                    drops: &drops,
                    sent: Cell::new(Some(sent)),
                    address: Cell::new(None),
                    panic_poll,
                    _local: Rc::new(()),
                    _pinned: PhantomPinned,
                });
                if panic_poll {
                    assert!(result.is_err());
                } else {
                    assert_eq!(result.unwrap(), text.len());
                }
                assert_eq!(drops.load(Ordering::SeqCst), 1);
                received.recv_timeout(TEST_TIMEOUT).unwrap()
            };
            std::thread::spawn(move || {
                for _ in 0..8 {
                    retained.wake_by_ref();
                    let consuming = retained.clone();
                    consuming.wake();
                }
            })
            .join()
            .unwrap();
            assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
            runtime.stop().unwrap();
        }
    });
}
