// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::TypeId;
use std::error::Error;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::Waker;
use std::time::{Duration, Instant};
use std::{io, thread};

use arty_io_core::{CompletionBudget, CompletionWaiter, DriverContext, DriverError, DriverProvider, IoContext, SystemTasks};
use thread_aware_core::ThreadAware;

use super::coordinator::Source;
use super::echo_driver::{EchoContext, EchoIoError};
use super::sample_driver::{SampleContext, SampleIoError};
use super::test_support::Harness;

fn budget(units: usize) -> CompletionBudget {
    CompletionBudget::new(NonZeroUsize::new(units).unwrap())
}

#[test]
fn both_models_require_collection_then_owner_service() {
    let mut harness = Harness::new(2);
    let (mut sample_driver, _) = harness.create::<SampleContext>();
    let (mut echo_driver, _) = harness.create::<EchoContext>();
    let sample = sample_driver.context();
    let echo = echo_driver.context();
    let number = sample.submit(41).unwrap();
    let text = echo.submit("arty").unwrap();
    sample_driver.service(&mut budget(2)).unwrap();
    echo_driver.service(&mut budget(2)).unwrap();
    assert!(number.is_pending());
    assert!(text.is_pending());

    harness.coordinator.waiter.collect(Duration::ZERO, &mut budget(2)).unwrap();
    assert!(number.is_pending());
    assert!(text.is_pending());
    sample_driver.service(&mut budget(1)).unwrap();
    echo_driver.service(&mut budget(1)).unwrap();
    assert_eq!(number.wait().unwrap(), 42);
    assert_eq!(text.wait().unwrap(), "ARTY");
    assert_eq!(sample.completed_on(), Some(thread::current().id()));
    assert_eq!(echo.completed_on(), sample.completed_on());
    assert_eq!(harness.coordinator.waiter.metrics().owner, sample.driver_thread());
}

#[test]
fn budget_exhaustion_retains_each_models_continuation() {
    let mut harness = Harness::new(1);
    let (sample_driver, sample_source) = harness.create::<SampleContext>();
    let (echo_driver, echo_source) = harness.create::<EchoContext>();
    let sample = sample_driver.context();
    let echo = echo_driver.context();
    let mut numbers = Vec::new();
    let mut texts = Vec::new();
    for input in 0..4 {
        numbers.push(sample.submit(input).unwrap());
        texts.push(echo.submit("q").unwrap());
    }
    harness.coordinator.waiter.collect(Duration::ZERO, &mut budget(8)).unwrap();
    harness
        .coordinator
        .insert(TypeId::of::<SampleContext>(), sample_source, Box::new(sample_driver));
    harness
        .coordinator
        .insert(TypeId::of::<EchoContext>(), echo_source, Box::new(echo_driver));
    for completed in 1..=4 {
        harness.coordinator.service(Instant::now());
        assert_eq!(numbers.iter().filter(|operation| !operation.is_pending()).count(), completed);
        assert_eq!(texts.iter().filter(|operation| !operation.is_pending()).count(), completed);
        if completed != 4 {
            assert_eq!(harness.coordinator.wait_duration(Instant::now(), None), Duration::ZERO);
        }
    }
    for (input, operation) in numbers.into_iter().enumerate() {
        assert_eq!(operation.wait().unwrap(), input + 1);
    }
    for operation in texts {
        assert_eq!(operation.wait().unwrap(), "Q");
    }
}

#[test]
fn retained_contexts_do_not_delay_concurrent_drains() {
    let mut harness = Harness::new(2);
    let sample = harness.install::<SampleContext>();
    let echo = harness.install::<EchoContext>();
    let number = sample.submit(8).unwrap();
    let text = echo.submit("drain").unwrap();
    harness.coordinator.begin_shutdown_all();
    assert_eq!(harness.tasks.len(), 2);
    assert!(matches!(sample.submit(1), Err(SampleIoError::Closed)));
    assert!(matches!(echo.submit("later"), Err(EchoIoError::Closed)));
    harness.coordinator.service(Instant::now());
    assert_eq!(number.wait().unwrap(), 9);
    assert_eq!(text.wait().unwrap(), "DRAIN");
    assert!(!harness.coordinator.is_empty());
    harness.tasks.run_all();
    harness.coordinator.service(Instant::now());
    assert!(harness.coordinator.is_empty());
    assert!(harness.coordinator.take_retired().iter().all(|retired| retired.errors.is_empty()));
    // Relaxed reads observe diagnostic counts after synchronous owner-thread retirement.
    let metrics = harness.coordinator.waiter.metrics();
    assert_eq!(metrics.records_retired.load(Ordering::Relaxed), 1);
    assert_eq!(metrics.readiness_retired.load(Ordering::Relaxed), 1);
}

#[test]
fn abandoned_drains_keep_late_packets_and_callbacks_safe() {
    let mut harness = Harness::new(8);
    let sample = harness.install::<SampleContext>();
    let echo = harness.install::<EchoContext>();
    let number = sample.submit(8).unwrap();
    let text = echo.submit("abandoned").unwrap();
    harness.coordinator.begin_shutdown_all();
    harness.coordinator.expire_all();
    let retired = harness.coordinator.take_retired();
    assert_eq!(retired.len(), 2);
    assert!(
        retired
            .iter()
            .all(|entry| entry.errors.len() == 1 && entry.errors[0].is_shutdown_timeout())
    );
    assert!(matches!(number.wait(), Err(SampleIoError::Abandoned)));
    assert!(matches!(text.wait(), Err(EchoIoError::Abandoned)));

    // These new registrations have the same context TypeIds, but not the old route identities.
    let replacement = harness.install::<SampleContext>();
    let replacement_echo = harness.install::<EchoContext>();
    harness.tasks.run_all();
    let number = replacement.submit(20).unwrap();
    let text = replacement_echo.submit("new").unwrap();
    harness.coordinator.service(Instant::now());
    assert_eq!(number.wait().unwrap(), 21);
    assert_eq!(text.wait().unwrap(), "NEW");
    assert_eq!(replacement.operation_count(), 1);
    assert!(matches!(sample.submit(1), Err(SampleIoError::Closed)));
    assert!(matches!(echo.submit("old"), Err(EchoIoError::Closed)));
}

#[test]
fn relocation_does_not_rebind_an_operation() {
    let mut harness = Harness::new(2);
    let mut context = harness.install::<SampleContext>();
    let operation = context.submit(3).unwrap();
    let owner = context.driver_thread();
    let destination = thread_aware_core::__private::v1::new_thread(
        thread_aware_core::__private::v1::new_owner(),
        thread::current().id(),
        thread_aware_core::__private::v1::new_numa_node(7),
    );
    context.relocate(None, &destination);
    harness.coordinator.service(Instant::now());
    assert_eq!(operation.wait().unwrap(), 4);
    assert_eq!(context.driver_thread(), owner);
}

#[test]
fn losing_interest_and_operation_errors_do_not_fail_the_driver() {
    let mut harness = Harness::new(4);
    let sample = harness.install::<SampleContext>();
    let echo = harness.install::<EchoContext>();
    drop(sample.submit(7).unwrap());
    drop(echo.submit("unobserved").unwrap());
    let overflow = sample.submit(usize::MAX).unwrap();
    harness.coordinator.service(Instant::now());
    assert!(matches!(overflow.wait(), Err(SampleIoError::Overflow)));
    assert!(!harness.coordinator.has_failed());
    harness.coordinator.begin_shutdown_all();
    harness.tasks.run_all();
    harness.coordinator.service(Instant::now());
    assert!(harness.coordinator.is_empty());
}

#[test]
fn missing_actual_clients_fail_before_native_registration_or_system_work() {
    let harness = Harness::new(1);
    let context = || DriverContext::new(harness.thread.clone(), harness.tasks.handle(), Waker::noop().clone());
    let sample = SampleContext::provider().unwrap().create(context());
    let echo = EchoContext::provider().unwrap().create(context());
    assert!(sample.err().unwrap().is_unsupported());
    assert!(echo.err().unwrap().is_unsupported());
    let metrics = harness.coordinator.waiter.metrics();
    assert_eq!(metrics.records_created.load(Ordering::Relaxed), 0);
    assert_eq!(metrics.readiness_created.load(Ordering::Relaxed), 0);
    assert_eq!(harness.tasks.len(), 0);
}

#[test]
fn rejected_cleanup_is_reported_on_the_first_drain_turn_with_active_operations() {
    let harness = Harness::new(2);
    let tasks = SystemTasks::new(|task| {
        drop(task);
        Err(DriverError::unsupported("cleanup submission rejected")
            .with_cause(io::Error::new(io::ErrorKind::PermissionDenied, "executor admission denied")))
    });
    let context = || {
        harness
            .coordinator
            .waiter
            .attach_clients(DriverContext::new(harness.thread.clone(), tasks.clone(), Waker::noop().clone()))
            .unwrap()
    };
    let sample_driver = SampleContext::provider().unwrap().create(context()).unwrap();
    let echo_driver = EchoContext::provider().unwrap().create(context()).unwrap();
    let sample = sample_driver.context();
    let echo = echo_driver.context();
    let number = sample.submit(9).unwrap();
    let text = echo.submit("pending").unwrap();
    let drains = [sample_driver.shutdown(), echo_driver.shutdown()];
    assert!(matches!(sample.submit(0), Err(SampleIoError::Closed)));
    assert!(matches!(echo.submit("closed"), Err(EchoIoError::Closed)));
    assert!(number.is_pending());
    assert!(text.is_pending());

    // No native collection has occurred: both drivers still have admitted, incomplete work.
    for mut drain in drains {
        let error = drain.service(&mut budget(1)).unwrap_err();
        assert!(error.is_unsupported());
        assert_eq!(error.to_string(), "cleanup submission rejected");
        assert_eq!(
            error.source().unwrap().downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
        drop(drain);
    }
    assert!(matches!(number.wait(), Err(SampleIoError::Abandoned)));
    assert!(matches!(text.wait(), Err(EchoIoError::Abandoned)));
}

#[test]
fn cleanup_rejection_does_not_stop_an_independent_drain() {
    let mut harness = Harness::new(2);
    let source = Source::new(harness.coordinator.waiter.waker());
    let tasks = SystemTasks::new(|task| {
        drop(task);
        Err(DriverError::from_message("cleanup submission rejected"))
    });
    let context = harness
        .coordinator
        .waiter
        .attach_clients(DriverContext::new(harness.thread.clone(), tasks, Waker::from(Arc::clone(&source))))
        .unwrap();
    let driver = SampleContext::provider().unwrap().create(context).unwrap();
    let sample = driver.context();
    harness.coordinator.insert(TypeId::of::<SampleContext>(), source, Box::new(driver));
    let echo = harness.install::<EchoContext>();
    let number = sample.submit(1).unwrap();
    let text = echo.submit("independent").unwrap();
    harness.coordinator.begin_shutdown_all();
    assert_eq!(harness.tasks.len(), 1);
    harness.coordinator.service(Instant::now());
    assert!(harness.coordinator.has_failed());
    assert!(!harness.coordinator.is_empty());
    let retired = harness.coordinator.take_retired();
    assert_eq!(retired.len(), 1);
    assert_eq!(retired[0].id, TypeId::of::<SampleContext>());
    assert_eq!(retired[0].errors.len(), 1);
    assert_eq!(retired[0].errors[0].to_string(), "cleanup submission rejected");
    assert!(matches!(number.wait(), Err(SampleIoError::Abandoned)));

    harness.tasks.run_all();
    harness.coordinator.service(Instant::now());
    assert!(harness.coordinator.is_empty());
    let retired = harness.coordinator.take_retired();
    assert_eq!(retired.len(), 1);
    assert_eq!(retired[0].id, TypeId::of::<EchoContext>());
    assert!(retired[0].errors.is_empty());
    assert_eq!(text.wait().unwrap(), "INDEPENDENT");
}
