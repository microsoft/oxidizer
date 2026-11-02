// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction errors.

#![cfg(feature = "rt")]
// Under Miri, test-util supplies a processor model for the same selection logic.

use std::error::Error as StdError;

use arty::runtime::{BlockingPoolPolicy, Error, MonitorPolicy, PanicPolicy, Runtime, WorkersPolicy};

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
    for policy in [PanicPolicy::shutdown(), PanicPolicy::ignore()] {
        let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).panic(policy).build().unwrap();

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
