// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A rejected borrowing factory releases caller storage even when its Drop unwinds.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};

use arty::runtime::RuntimeOperations;
use panic_support::{isolated, runtime};

testing_aids::init_tracing!();

struct BorrowedCapture<'a>(&'a AtomicUsize);

impl Drop for BorrowedCapture<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("rejected borrowing factory destructor");
    }
}

#[test]
fn stopped_borrowing_factory_is_destroyed_before_its_drop_unwind_escapes() {
    isolated("stopped_borrowing_factory_is_destroyed_before_its_drop_unwind_escapes", || {
        let runtime = runtime();
        RuntimeOperations::from(&runtime).request_stop();
        let drops = AtomicUsize::new(0);
        let invoked = AtomicUsize::new(0);
        let invoked = &invoked;
        let capture = BorrowedCapture(&drops);
        let result = catch_unwind(AssertUnwindSafe(|| {
            runtime.scheduler().block_on(move |_| {
                invoked.fetch_add(1, Ordering::SeqCst);
                drop(capture);
                std::future::ready(())
            })
        }));
        result.unwrap_err();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(invoked.load(Ordering::SeqCst), 0);
        runtime.stop().unwrap();
    });
}

#[test]
fn nested_borrowing_factory_drop_unwind_does_not_submit_work() {
    isolated("nested_borrowing_factory_drop_unwind_does_not_submit_work", || {
        let runtime = runtime();
        let drops = AtomicUsize::new(0);
        let invoked = AtomicUsize::new(0);
        let invoked = &invoked;
        let capture = BorrowedCapture(&drops);
        let result = futures::executor::block_on(async {
            catch_unwind(AssertUnwindSafe(|| {
                runtime.scheduler().block_on(move |_| {
                    invoked.fetch_add(1, Ordering::SeqCst);
                    drop(capture);
                    std::future::ready(())
                })
            }))
        });
        result.unwrap_err();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(invoked.load(Ordering::SeqCst), 0);
        assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
        runtime.stop().unwrap();
    });
}
