// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction errors and startup telemetry.

#![cfg(feature = "rt")]
// Under Miri, test-util supplies a processor model for the same selection logic.

testing_aids::init_tracing!();

use std::error::Error as StdError;

use arty::runtime::{BlockingPoolPolicy, Error, Runtime, WorkersPolicy};
use observed_testing::{CapturedEvent, TEST_ID, test_emitter};

#[test]
fn zero_counts_are_rejected_when_the_runtime_is_built() {
    const EXACT: WorkersPolicy = WorkersPolicy::exactly(0);
    const MAXIMUM: WorkersPolicy = WorkersPolicy::at_most(0);

    for policy in [EXACT, MAXIMUM] {
        let (sink, processor) = test_emitter(TEST_ID);
        let builder = Runtime::builder().workers(policy).sink(sink);
        let error: Error = builder.build().unwrap_err();

        assert_eq!(error.to_string(), "worker count must be greater than zero");
        assert!(error.source().is_some());
        let events = processor.events();
        let names: Vec<_> = events.iter().map(CapturedEvent::name).collect();
        assert_eq!(names, vec!["arty.rt.start_failed"]);
    }
}

#[test]
fn zero_shared_blocking_limits_are_rejected_when_the_runtime_is_built() {
    for policy in [BlockingPoolPolicy::shared().max(0), BlockingPoolPolicy::per_worker().max(0)] {
        let (sink, processor) = test_emitter(TEST_ID);
        let error: Error = Runtime::builder()
            .workers(WorkersPolicy::at_most(1))
            .blocking_pool(policy)
            .sink(sink)
            .build()
            .unwrap_err();

        assert_eq!(error.to_string(), "blocking pool max_workers must be greater than zero");
        assert!(error.source().is_some());
        let events = processor.events();
        let names: Vec<_> = events.iter().map(CapturedEvent::name).collect();
        assert_eq!(names, vec!["arty.rt.start_failed"]);
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

#[test]
fn unavailable_processors_return_a_typed_construction_error() {
    let error: Error = Runtime::builder().workers(WorkersPolicy::exactly(usize::MAX)).build().unwrap_err();
    assert!(error.to_string().contains(&usize::MAX.to_string()));
    assert!(error.source().is_some());
}

#[test]
fn processor_selection_failure_reports_failure_without_starting_threads() {
    let (sink, processor) = test_emitter(TEST_ID);

    Runtime::builder()
        .workers(WorkersPolicy::exactly(usize::MAX))
        .sink(sink)
        .build()
        .unwrap_err();

    let events = processor.events();
    let names: Vec<_> = events.iter().map(CapturedEvent::name).collect();
    assert_eq!(names, vec!["arty.rt.start_failed"]);
}

#[test]
fn runtime_can_be_constructed_after_a_rejected_processor_request() {
    Runtime::builder().workers(WorkersPolicy::exactly(usize::MAX)).build().unwrap_err();

    let runtime = Runtime::builder().workers(WorkersPolicy::at_most(1)).build().unwrap();

    assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
}
