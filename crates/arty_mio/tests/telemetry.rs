// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public telemetry behavior for Mio lifecycle and failure events.

#![allow(clippy::unwrap_used, reason = "test code")]

testing_aids::init_tracing!();

use std::io;
use std::net::UdpSocket as StdUdpSocket;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use arty_io_core::{Cycle, Driver, DriverOptions, DriverRole, IoContext};
use arty_mio::{MioContext, MioDriver, MioOptions};
use mio::event::Source;
use mio::net::UdpSocket;
use mio::{Interest, Registry, Token};
use observed::Value;
use observed_testing::{MockProcessor, TEST_ID, test_emitter};
use thread_aware::ThreadBuilder;
use thread_aware_core::ThreadAware;

#[test]
fn lifecycle_events_emit_once_per_driver() {
    for (role, role_label) in [(DriverRole::Primary, "primary"), (DriverRole::Secondary, "secondary")] {
        let (sink, processor) = test_emitter(TEST_ID);
        let (_io, driver) = driver(MioOptions::default().sink(sink), role);

        assert_eq!(event_names(&processor), ["arty.mio.driver.started"]);

        driver.shutdown().unwrap();

        assert_eq!(event_names(&processor), ["arty.mio.driver.started", "arty.mio.driver.stopped"]);
        let events = processor.events();
        assert_dimension(&events[0], "io.driver.role", role_label);
        assert_dimension(&events[1], "outcome", "succeeded");
    }
}

#[test]
fn registration_failure_is_classified_and_reported_once() {
    let (sink, processor) = test_emitter(TEST_ID);
    let (io, driver) = driver(MioOptions::default().sink(sink), DriverRole::Primary);

    for _ in 0..2 {
        let error = io.register(FailingRegisterSource, Interest::READABLE).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::Other);
    }

    let failures = events_named(&processor, "arty.mio.source.operation_failed");
    assert_eq!(failures.len(), 1);
    assert_dimension(&failures[0], "operation", "registration");
    assert_dimension(&failures[0], "error.kind", "Other");
    assert_dimension(&failures[0], "error.message", "scripted registration failure");

    driver.shutdown().unwrap();
}

#[test]
fn deregistration_failure_reports_the_source_and_driver_boundaries() {
    let (sink, processor) = test_emitter(TEST_ID);
    let (io, mut driver) = driver(MioOptions::default().sink(sink), DriverRole::Primary);
    let registration = io.register(FailingDeregisterSource, Interest::READABLE).unwrap();

    drop(registration);
    let error = driver.execute_cycle(&mut Cycle::new(Duration::ZERO)).unwrap_err();
    assert_eq!(error.to_string(), "scripted deregistration failure");

    let source_failure = events_named(&processor, "arty.mio.source.operation_failed");
    assert_eq!(source_failure.len(), 1);
    assert_dimension(&source_failure[0], "operation", "deregistration");
    assert_dimension(&source_failure[0], "error.kind", "Other");

    let driver_failure = events_named(&processor, "arty.mio.driver.failed");
    assert_eq!(driver_failure.len(), 1);
    assert_dimension(&driver_failure[0], "operation", "cycle");
    assert_dimension(&driver_failure[0], "error.kind", "Other");

    driver.shutdown().unwrap();
}

#[test]
fn unattached_context_failure_is_reported_once() {
    let (sink, processor) = test_emitter(TEST_ID);
    let io = MioContext::context(MioOptions::default().sink(sink));

    for _ in 0..2 {
        let error = io.register(FailingRegisterSource, Interest::READABLE).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotConnected);
    }

    let failures = events_named(&processor, "arty.mio.context.attachment_failed");
    assert_eq!(failures.len(), 1);
    assert_dimension(&failures[0], "error.kind", "NotConnected");
}

#[test]
fn successful_registration_and_readiness_emit_no_events() {
    let (sink, processor) = test_emitter(TEST_ID);
    let (io, mut driver) = driver(MioOptions::default().sink(sink), DriverRole::Primary);
    let receiver = UdpSocket::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let sender = StdUdpSocket::bind("127.0.0.1:0").unwrap();
    sender.connect(receiver.local_addr().unwrap()).unwrap();
    let registration = io.register(receiver, Interest::READABLE).unwrap();

    sender.send(&[1]).unwrap();
    driver.execute_cycle(&mut Cycle::new(Duration::from_secs(1))).unwrap();
    assert!(poll_read_ready(&registration).is_ready());
    drop(registration);

    assert_eq!(event_names(&processor), ["arty.mio.driver.started"]);
    driver.shutdown().unwrap();
    assert_eq!(event_names(&processor), ["arty.mio.driver.started", "arty.mio.driver.stopped"]);
}

fn driver(options: MioOptions, role: DriverRole) -> (MioContext, MioDriver) {
    let mut io = MioContext::context(options);
    let worker = ThreadBuilder::default().build(std::thread::current().id());
    io.relocate(None, &worker);
    let driver = io.driver(DriverOptions::new(worker, Waker::noop().clone(), role)).unwrap();
    (io, driver)
}

fn poll_read_ready<S>(registration: &arty_mio::Registered<S>) -> Poll<Result<(), arty_mio::RegistrationClosed>>
where
    S: Source + Send + 'static,
{
    registration.poll_read_ready(&mut Context::from_waker(Waker::noop()))
}

fn event_names(processor: &MockProcessor) -> Vec<String> {
    processor.events().iter().map(|event| event.name().to_owned()).collect()
}

fn events_named(processor: &MockProcessor, name: &str) -> Vec<observed_testing::CapturedEvent> {
    processor.events().into_iter().filter(|event| event.name() == name).collect()
}

fn assert_dimension(event: &observed_testing::CapturedEvent, key: &str, expected: &str) {
    let value = event
        .dimensions()
        .into_iter()
        .find_map(|(field, value)| (field == key).then_some(value))
        .unwrap();
    assert_eq!(value, Value::from(expected.to_owned()));
}

struct FailingRegisterSource;

impl Source for FailingRegisterSource {
    fn register(&mut self, _registry: &Registry, _token: Token, _interests: Interest) -> io::Result<()> {
        Err(io::Error::other("scripted registration failure"))
    }

    fn reregister(&mut self, _registry: &Registry, _token: Token, _interests: Interest) -> io::Result<()> {
        unreachable!("the test source never registers")
    }

    fn deregister(&mut self, _registry: &Registry) -> io::Result<()> {
        unreachable!("the test source never registers")
    }
}

struct FailingDeregisterSource;

impl Source for FailingDeregisterSource {
    fn register(&mut self, _registry: &Registry, _token: Token, _interests: Interest) -> io::Result<()> {
        Ok(())
    }

    fn reregister(&mut self, _registry: &Registry, _token: Token, _interests: Interest) -> io::Result<()> {
        Ok(())
    }

    fn deregister(&mut self, _registry: &Registry) -> io::Result<()> {
        Err(io::Error::other("scripted deregistration failure"))
    }
}
