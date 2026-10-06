// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public shutdown guards must prevent an async worker from waiting on itself.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use panic_support::{isolated, runtime};

testing_aids::init_tracing!();

#[test]
#[cfg_attr(
    miri,
    ignore = "self-stop intentionally returns before the worker can finish; native and careful suites retain the guard contract"
)]
fn async_worker_stop_returns_without_self_joining() {
    isolated("async_worker_stop_returns_without_self_joining", || {
        let runtime = runtime();
        let scheduler = runtime
            .scheduler()
            .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
            .wait()
            .unwrap();
        let result = scheduler.spawn(async move |_| runtime.stop()).wait().unwrap();
        assert!(result.is_err());
    });
}
