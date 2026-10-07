// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Direct same-pool blocking joins must be rejected instead of starving their pool.

#![cfg(feature = "rt")]
#![cfg(not(miri))]

mod panic_support;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;

use panic_support::runtime;
use testing_aids::{TEST_TIMEOUT, is_mutation_testing};

testing_aids::init_tracing!();

fn completes_without_deadlock(body: impl FnOnce() + Send + 'static) {
    let (completed, completion) = mpsc::channel();
    std::thread::spawn(move || {
        body();
        _ = completed.send(());
    });
    let deadlocked = if is_mutation_testing() {
        completion.recv().is_err()
    } else {
        completion.recv_timeout(TEST_TIMEOUT).is_err()
    };
    if deadlocked {
        eprintln!("the supported operation deadlocked");
        #[expect(clippy::exit, reason = "a deadlocked runtime thread prevents normal child-process teardown")]
        std::process::exit(112);
    }
}

#[test]
fn blocking_join_future_rejects_its_current_pool() {
    completes_without_deadlock(|| {
        let runtime = runtime();
        let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
        let nested = scheduler.clone();
        let task = scheduler.spawn_blocking(move || {
            let inner = nested.spawn_blocking(|| 42);
            catch_unwind(AssertUnwindSafe(|| futures::executor::block_on(inner))).is_err()
        });

        assert!(futures::executor::block_on(task).unwrap());
        runtime.stop().unwrap();
    });
}
