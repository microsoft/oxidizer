// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction errors and startup telemetry.

#![cfg(feature = "rt")]
// Processor discovery uses native hardware APIs that Miri cannot provide.
#![cfg(not(miri))]

testing_aids::init_tracing!();

use std::error::Error as StdError;
use std::num::NonZeroUsize;

use arty::rt::config::{BuildError, ProcessorCount};
use arty::rt::{Error, Runtime};
use observed_testing::{CapturedEvent, TEST_ID, test_emitter};

#[test]
fn unavailable_processors_return_a_typed_construction_error() {
    let error: BuildError = Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::MAX))
        .build()
        .unwrap_err();
    let error = Error::from(error);

    assert!(error.source().unwrap().is::<BuildError>());
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
