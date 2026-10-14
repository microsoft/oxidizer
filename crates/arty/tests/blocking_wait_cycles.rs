// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Nested blocking joins complete when the pool has spare worker capacity.

#![cfg(feature = "rt")]
#![cfg(not(miri))]

use std::sync::mpsc;

use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
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
fn blocking_join_future_completes_on_its_current_pool() {
    completes_without_deadlock(|| {
        let runtime = Runtime::builder()
            .workers(WorkersPolicy::exactly(1))
            .blocking_pool(BlockingPoolPolicy::shared().max(2))
            .build()
            .unwrap();
        let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
        let nested = scheduler.clone();
        let task = scheduler.spawn_blocking(move || {
            let inner = nested.spawn_blocking(|| 42);
            futures::executor::block_on(inner).unwrap()
        });

        assert_eq!(futures::executor::block_on(task).unwrap(), 42);
        runtime.stop().unwrap();
    });
}
