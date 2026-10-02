// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Borrowing factories and pinned futures are destroyed before a root join returns.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::cell::Cell;
use std::error::Error as _;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use panic_support::{isolated, runtime};

testing_aids::init_tracing!();

struct BorrowedTask<'a> {
    drops: &'a AtomicUsize,
    address: Cell<Option<usize>>,
    panic_poll: bool,
    panic_drop: bool,
    _pinned: PhantomPinned,
}

impl Future for BorrowedTask<'_> {
    type Output = u32;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<u32> {
        let this = self.as_ref().get_ref();
        this.address.set(Some(std::ptr::from_ref(this).addr()));
        assert!(!this.panic_poll, "borrowed pinned task poll");
        Poll::Ready(42)
    }
}

impl Drop for BorrowedTask<'_> {
    fn drop(&mut self) {
        assert_eq!(Some(std::ptr::from_ref(self).addr()), self.address.get());
        self.drops.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic_drop, "borrowed pinned task destructor");
    }
}

#[test]
fn pinned_borrowing_future_teardown_precedes_root_return() {
    isolated("pinned_borrowing_future_teardown_precedes_root_return", || {
        let runtime = runtime();
        for (panic_poll, panic_drop) in [(false, false), (true, false), (false, true)] {
            let drops = AtomicUsize::new(0);
            let result = runtime.scheduler().block_on(|_| BorrowedTask {
                drops: &drops,
                address: Cell::new(None),
                panic_poll,
                panic_drop,
                _pinned: PhantomPinned,
            });
            assert_eq!(drops.load(Ordering::SeqCst), 1);
            if panic_poll || panic_drop {
                assert!(
                    result
                        .unwrap_err()
                        .source()
                        .unwrap()
                        .downcast_ref::<arty::task::JoinError>()
                        .unwrap()
                        .is_panic()
                );
            } else {
                assert_eq!(result.unwrap(), 42);
            }
            assert_eq!(runtime.scheduler().block_on(async |_| 7).unwrap(), 7);
        }
        runtime.stop().unwrap();
    });
}

#[test]
fn borrowed_factory_capture_drop_panic_is_reported_after_destruction() {
    isolated("borrowed_factory_capture_drop_panic_is_reported_after_destruction", || {
        struct Capture<'a>(&'a AtomicUsize);

        impl Drop for Capture<'_> {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
                panic!("borrowed factory capture destructor");
            }
        }

        let runtime = runtime();
        let drops = AtomicUsize::new(0);
        let capture = Capture(&drops);
        let result = runtime.scheduler().block_on(move |_| {
            drop(capture);
            std::future::ready(42)
        });
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(
            result
                .unwrap_err()
                .source()
                .unwrap()
                .downcast_ref::<arty::task::JoinError>()
                .unwrap()
                .is_panic()
        );
        assert_eq!(runtime.scheduler().block_on(async |_| 7).unwrap(), 7);
        runtime.stop().unwrap();
    });
}
