// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime configuration and construction errors.

#![cfg(feature = "rt")]
// Under Miri, `test-util` supplies a processor model for the same selection logic.

use std::error::Error as StdError;
use std::thread;

use arty::runtime::hook::Panic;
use arty::runtime::{BlockingPoolPolicy, Error, MonitorPolicy, Runtime, WorkersPolicy};

fn worker_names(runtime: &Runtime) -> Vec<(String, String)> {
    runtime
        .scheduler()
        .block_on(async |cx| {
            let mut names = Vec::new();
            for worker in cx.control().workers() {
                let pair = cx
                    .scheduler()
                    .spawn_at(worker, cx.scheduler().clone(), |scheduler| async move {
                        let async_name = thread::current()
                            .name()
                            .expect("Arty async worker threads always have names")
                            .to_owned();
                        let blocking_name = scheduler
                            .spawn_blocking(|| {
                                thread::current()
                                    .name()
                                    .expect("Arty blocking worker threads always have names")
                                    .to_owned()
                            })
                            .await
                            .expect("the runtime remains active while collecting blocking worker names");
                        (async_name, blocking_name)
                    })
                    .await
                    .expect("the runtime remains active while collecting async worker names");
                names.push(pair);
            }
            names
        })
        .expect("the runtime remains active while collecting worker names")
}

#[test]
fn default_worker_names_use_arty() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();

    assert_eq!(
        worker_names(&runtime),
        vec![
            ("arty-async-0".to_owned(), "arty-blocking-0".to_owned()),
            ("arty-async-1".to_owned(), "arty-blocking-0".to_owned()),
        ]
    );
    runtime.stop().unwrap();
}

#[test]
fn runtime_name_is_copied_and_replaces_the_previous_name() {
    for (policy, blocking_names) in [
        (BlockingPoolPolicy::shared(), ["requests-blocking-0", "requests-blocking-0"]),
        (BlockingPoolPolicy::per_worker(), ["requests-blocking-0-0", "requests-blocking-1-0"]),
    ] {
        let name = String::from("requests");
        let builder = Runtime::builder()
            .name("invalid\0name")
            .name(&name)
            .workers(WorkersPolicy::exactly(2))
            .blocking_pool(policy.max(1));
        drop(name);
        let runtime = builder.build().unwrap();

        assert_eq!(
            worker_names(&runtime),
            vec![
                ("requests-async-0".to_owned(), blocking_names[0].to_owned()),
                ("requests-async-1".to_owned(), blocking_names[1].to_owned()),
            ]
        );
        runtime.stop().unwrap();
    }
}

#[test]
fn runtime_names_are_independent() {
    let first = Runtime::builder().name(String::from("first")).build().unwrap();
    let second = Runtime::builder().name("second").build().unwrap();

    assert_eq!(
        worker_names(&first),
        vec![("first-async-0".to_owned(), "first-blocking-0".to_owned())]
    );
    assert_eq!(
        worker_names(&second),
        vec![("second-async-0".to_owned(), "second-blocking-0".to_owned())]
    );
    first.stop().unwrap();
    second.stop().unwrap();
}

#[test]
fn empty_runtime_name_is_accepted() {
    let runtime = Runtime::builder().name("").build().unwrap();

    assert_eq!(worker_names(&runtime), vec![("-async-0".to_owned(), "-blocking-0".to_owned())]);
    runtime.stop().unwrap();
}

#[test]
fn null_in_runtime_name_is_rejected_when_built() {
    for policy in [BlockingPoolPolicy::shared(), BlockingPoolPolicy::per_worker()] {
        let error = Runtime::builder().name("invalid\0name").blocking_pool(policy).build().unwrap_err();

        assert_eq!(error.to_string(), "runtime name must not contain a null character");
        assert!(error.source().is_some());
    }
}

#[test]
fn monitor_policy_defaults_to_enabled() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .monitor(MonitorPolicy::default())
        .build()
        .unwrap();

    assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
    runtime.stop().unwrap();
}

#[test]
fn explicit_monitor_policies_preserve_runtime_behavior() {
    for policy in [MonitorPolicy::disabled(), MonitorPolicy::enabled()] {
        let runtime = Runtime::builder()
            .workers(WorkersPolicy::exactly(1))
            .monitor(policy)
            .build()
            .unwrap();

        assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
        runtime.stop().unwrap();
    }
}

#[test]
fn default_runtime_construction_uses_one_worker() {
    for runtime in [
        Runtime::new().unwrap(),
        Runtime::builder().build().unwrap(),
        Runtime::builder().workers(WorkersPolicy::default()).build().unwrap(),
    ] {
        runtime
            .scheduler()
            .block_on(async |cx| {
                let workers: Vec<_> = cx.control().workers().into_iter().collect();
                assert_eq!(workers, vec![cx.thread()]);
            })
            .unwrap();
        runtime.stop().unwrap();
    }
}

#[test]
fn explicit_panic_policies_preserve_runtime_construction() {
    for builder in [
        Runtime::builder().workers(WorkersPolicy::exactly(1)).on(Panic::SHUTDOWN),
        Runtime::builder().workers(WorkersPolicy::exactly(1)).on(Panic::IGNORE),
    ] {
        let runtime = builder.build().unwrap();

        assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
        runtime.stop().unwrap();
    }
}

#[test]
fn zero_counts_are_rejected_when_the_runtime_is_built() {
    const EXACT: WorkersPolicy = WorkersPolicy::exactly(0);
    const MAXIMUM: WorkersPolicy = WorkersPolicy::at_most(0);

    for policy in [EXACT, MAXIMUM] {
        let builder = Runtime::builder().workers(policy);
        let error: Error = builder.build().unwrap_err();

        assert_eq!(error.to_string(), "worker count must be greater than zero");
        assert!(error.source().is_some());
    }
}

#[test]
fn zero_shared_blocking_limits_are_rejected_when_the_runtime_is_built() {
    for policy in [BlockingPoolPolicy::shared().max(0), BlockingPoolPolicy::per_worker().max(0)] {
        let error: Error = Runtime::builder()
            .workers(WorkersPolicy::at_most(1))
            .blocking_pool(policy)
            .build()
            .unwrap_err();

        assert_eq!(error.to_string(), "blocking pool max_workers must be greater than zero");
        assert!(error.source().is_some());
    }
}

#[test]
fn zero_shared_blocking_limit_can_be_replaced_before_building() {
    let runtime = Runtime::builder()
        .blocking_pool(BlockingPoolPolicy::shared().max(0))
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .build()
        .unwrap();

    runtime.stop().unwrap();
}
