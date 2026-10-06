// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction errors and startup telemetry.

#![cfg(feature = "rt")]
// Under Miri, test-util supplies a processor model for the same selection logic.

testing_aids::init_tracing!();

use std::error::Error as StdError;

use arty::runtime::{CpuPolicy, Error, Runtime};
use observed_testing::{CapturedEvent, TEST_ID, test_emitter};

#[test]
fn zero_counts_are_rejected_when_the_runtime_is_built() {
    const EXACT: CpuPolicy = CpuPolicy::exactly(0);
    const MAXIMUM: CpuPolicy = CpuPolicy::at_most(0);

    for policy in [EXACT, MAXIMUM] {
        let (sink, processor) = test_emitter(TEST_ID);
        let builder = Runtime::builder().cpu_policy(policy).sink(sink);
        let error: Error = builder.build().unwrap_err();

        assert_eq!(error.to_string(), "processor count must be greater than zero");
        assert!(error.source().is_some());
        let events = processor.events();
        let names: Vec<_> = events.iter().map(CapturedEvent::name).collect();
        assert_eq!(names, vec!["arty.rt.start_failed"]);
    }
}

#[test]
fn unavailable_processors_return_a_typed_construction_error() {
    let error: Error = Runtime::builder().cpu_policy(CpuPolicy::exactly(usize::MAX)).build().unwrap_err();
    assert!(error.to_string().contains(&usize::MAX.to_string()));
    assert!(error.source().is_some());
}

#[test]
fn processor_selection_failure_reports_failure_without_starting_threads() {
    let (sink, processor) = test_emitter(TEST_ID);

    Runtime::builder()
        .cpu_policy(CpuPolicy::exactly(usize::MAX))
        .sink(sink)
        .build()
        .unwrap_err();

    let events = processor.events();
    let names: Vec<_> = events.iter().map(CapturedEvent::name).collect();
    assert_eq!(names, vec!["arty.rt.start_failed"]);
}

#[test]
fn runtime_can_be_constructed_after_a_rejected_processor_request() {
    Runtime::builder().cpu_policy(CpuPolicy::exactly(usize::MAX)).build().unwrap_err();

    let runtime = Runtime::builder().cpu_policy(CpuPolicy::at_most(1)).build().unwrap();

    assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
}
