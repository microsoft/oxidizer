// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction errors and startup telemetry.

#![cfg(feature = "rt")]
// Under Miri, test-util supplies a processor model for the same selection logic.

testing_aids::init_tracing!();

use std::error::Error as StdError;
use std::num::NonZeroUsize;

use arty::runtime::{Error, ProcessorCount, Runtime};
use observed_testing::{CapturedEvent, TEST_ID, test_emitter};

#[test]
fn unavailable_processors_return_a_typed_construction_error() {
    let error: Error = Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::MAX))
        .build()
        .unwrap_err();
    assert!(error.to_string().contains(&usize::MAX.to_string()));
    assert!(error.source().is_some());
}

#[test]
fn processor_selection_failure_reports_failure_without_starting_threads() {
    let (sink, processor) = test_emitter(TEST_ID);

    Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::MAX))
        .sink(sink)
        .build()
        .unwrap_err();

    let events = processor.events();
    let names: Vec<_> = events.iter().map(CapturedEvent::name).collect();
    assert_eq!(names, vec!["oxidizer.rt.start_failed"]);
}

#[test]
fn runtime_can_be_constructed_after_a_rejected_processor_request() {
    Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::MAX))
        .build()
        .unwrap_err();

    let runtime = Runtime::builder()
        .processor_count(ProcessorCount::at_most(NonZeroUsize::MIN))
        .build()
        .unwrap();

    assert_eq!(runtime.run(async |_| 42), 42);
}
