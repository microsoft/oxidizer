// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public behavior for Mio ownership, registration, readiness, and shutdown.

#![allow(clippy::unwrap_used, reason = "test code")]
#![expect(clippy::panic, reason = "the polling helper reports a failed test precondition")]

use std::io;
use std::io::Write as _;
use std::net::{TcpListener, TcpStream as StdTcpStream, UdpSocket as StdUdpSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::ThreadId;
use std::time::Duration;

use arty_io_core::{Cycle, Driver, DriverOptions, DriverRole, IoContext};
use arty_mio::{MioContext, MioDriver, MioOptions, Registered, RegistrationClosed};
use mio::event::Source;
use mio::net::{TcpStream, UdpSocket};
use mio::{Interest, Registry, Token};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware::ThreadBuilder;
use thread_aware_core::ThreadAware;

assert_impl_all!(MioContext: Clone, Send, Sync, ThreadAware);
assert_impl_all!(MioOptions: Clone, Send, Sync, Default, std::fmt::Debug);
assert_not_impl_any!(MioOptions: Copy);
assert_impl_all!(MioDriver: Driver);
assert_impl_all!(RegistrationClosed: Send, Sync, std::fmt::Debug, std::fmt::Display, std::error::Error);
assert_impl_all!(Registered<TcpStream>: Send, Unpin);

#[test]
fn read_readiness_is_consumed_and_rearms_after_would_block() {
    let (io, mut driver) = driver(DriverRole::Primary, Waker::noop().clone());
    let (receiver, sender) = udp_pair();
    let mut registration = io.register(receiver, Interest::READABLE).unwrap();

    sender.send(&[1]).unwrap();
    driver.execute_cycle(&mut Cycle::new(Duration::from_secs(1))).unwrap();
    expect_ready(poll_read_ready(&registration, Waker::noop())).unwrap();

    let mut buffer = [0; 8];
    assert_eq!(registration.source_mut().unwrap().recv(&mut buffer).unwrap(), 1);
    assert_eq!(
        registration.source_mut().unwrap().recv(&mut buffer).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(poll_read_ready(&registration, Waker::noop()).is_pending());

    sender.send(&[2]).unwrap();
    driver.execute_cycle(&mut Cycle::new(Duration::from_secs(1))).unwrap();
    expect_ready(poll_read_ready(&registration, Waker::noop())).unwrap();

    drop(registration);
    driver.shutdown().unwrap();
}

#[test]
fn readiness_before_first_poll_and_waiter_replacement_are_lossless() {
    let (io, mut driver) = driver(DriverRole::Primary, Waker::noop().clone());
    let (receiver, sender) = udp_pair();
    let mut registration = io.register(receiver, Interest::READABLE).unwrap();

    sender.send(&[1]).unwrap();
    driver.execute_cycle(&mut Cycle::new(Duration::from_secs(1))).unwrap();
    expect_ready(poll_read_ready(&registration, Waker::noop())).unwrap();
    let mut buffer = [0; 8];
    assert_eq!(registration.source_mut().unwrap().recv(&mut buffer).unwrap(), 1);
    assert_eq!(
        registration.source_mut().unwrap().recv(&mut buffer).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );

    let first = Arc::new(WakeCounter::default());
    let second = Arc::new(WakeCounter::default());
    assert!(poll_read_ready(&registration, &Waker::from(Arc::clone(&first))).is_pending());
    assert!(poll_read_ready(&registration, &Waker::from(Arc::clone(&second))).is_pending());

    sender.send(&[2]).unwrap();
    driver.execute_cycle(&mut Cycle::new(Duration::from_secs(1))).unwrap();
    assert_eq!((first.count(), second.count()), (0, 1));

    drop(registration);
    driver.shutdown().unwrap();
}

#[test]
fn readable_and_writable_waiters_are_independent() {
    let (io, mut driver) = driver(DriverRole::Primary, Waker::noop().clone());
    let (stream, mut peer) = tcp_pair();
    let registration = io.register(stream, Interest::READABLE | Interest::WRITABLE).unwrap();
    driver.execute_cycle(&mut Cycle::new(Duration::ZERO)).unwrap();
    let read_wake = Arc::new(WakeCounter::default());
    let write_wake = Arc::new(WakeCounter::default());

    assert!(poll_read_ready(&registration, &Waker::from(Arc::clone(&read_wake))).is_pending());
    expect_ready(poll_write_ready(&registration, Waker::noop())).unwrap();
    assert!(poll_write_ready(&registration, &Waker::from(Arc::clone(&write_wake))).is_pending());

    peer.write_all(&[1]).unwrap();
    driver.execute_cycle(&mut Cycle::new(Duration::from_secs(1))).unwrap();

    assert_eq!((read_wake.count(), write_wake.count()), (1, 0));
    expect_ready(poll_read_ready(&registration, Waker::noop())).unwrap();
    assert!(poll_write_ready(&registration, Waker::noop()).is_pending());

    drop(registration);
    driver.shutdown().unwrap();
}

#[test]
fn secondary_driver_dispatches_readiness_and_wakes_the_owner() {
    let (owner_tx, owner_rx) = mpsc::channel();
    let owner_waker = Waker::from(Arc::new(ChannelWake(owner_tx)));
    let (io, mut driver) = driver(DriverRole::Secondary, owner_waker);
    let (receiver, sender) = udp_pair();
    let registration = io.register(receiver, Interest::READABLE).unwrap();
    assert!(owner_rx.try_recv().is_err());

    driver.execute_cycle(&mut Cycle::new(Duration::ZERO)).unwrap();
    sender.send(&[1]).unwrap();
    owner_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    driver.execute_cycle(&mut Cycle::new(Duration::ZERO)).unwrap();

    expect_ready(poll_read_ready(&registration, Waker::noop())).unwrap();
    drop(registration);
    driver.shutdown().unwrap();
}

#[test]
fn cloned_registries_run_source_registration_on_the_caller_thread() {
    let current = std::thread::current().id();
    let (primary_io, primary_driver) = driver(DriverRole::Primary, Waker::noop().clone());
    let primary_source = recording_source();
    let (primary_tx, primary_rx) = mpsc::channel();
    let primary_registration = primary_io
        .register(primary_source.with_reporter(primary_tx), Interest::READABLE)
        .unwrap();
    assert_eq!(primary_rx.recv().unwrap(), current);

    let (secondary_io, secondary_driver) = driver(DriverRole::Secondary, Waker::noop().clone());
    let secondary_source = recording_source();
    let (secondary_tx, secondary_rx) = mpsc::channel();
    let secondary_registration = secondary_io
        .register(secondary_source.with_reporter(secondary_tx), Interest::READABLE)
        .unwrap();
    assert_eq!(secondary_rx.recv().unwrap(), current);

    drop(primary_registration);
    drop(secondary_registration);
    primary_driver.shutdown().unwrap();
    secondary_driver.shutdown().unwrap();
}

#[test]
fn drop_deregisters_through_the_cloned_registry_on_the_caller_thread() {
    let current = std::thread::current().id();
    let (primary_io, primary_driver) = driver(DriverRole::Primary, Waker::noop().clone());
    let primary_source = recording_source();
    let (primary_tx, primary_rx) = mpsc::channel();
    let primary_registration = primary_io
        .register(primary_source.with_deregister_reporter(primary_tx), Interest::READABLE)
        .unwrap();

    drop(primary_registration);
    assert_eq!(primary_rx.recv().unwrap(), current);

    let (secondary_io, secondary_driver) = driver(DriverRole::Secondary, Waker::noop().clone());
    let secondary_source = recording_source();
    let (secondary_tx, secondary_rx) = mpsc::channel();
    let secondary_registration = secondary_io
        .register(secondary_source.with_deregister_reporter(secondary_tx), Interest::READABLE)
        .unwrap();

    drop(secondary_registration);
    assert_eq!(secondary_rx.recv().unwrap(), current);

    primary_driver.shutdown().unwrap();
    secondary_driver.shutdown().unwrap();
}

#[test]
fn context_clone_registers_on_the_calling_thread_and_relocation_selects_destination_worker() {
    let (mut io, driver) = driver(DriverRole::Primary, Waker::noop().clone());
    let external = io.clone();
    let (thread_tx, thread_rx) = mpsc::channel();
    let source = recording_source().with_reporter(thread_tx);

    let registration = std::thread::scope(|scope| {
        let task = scope.spawn(move || external.register(source, Interest::READABLE));
        task.join().unwrap().unwrap()
    });
    assert_ne!(thread_rx.recv().unwrap(), std::thread::current().id());
    drop(registration);

    std::thread::scope(|scope| {
        let mut relocated = io.clone();
        scope
            .spawn(move || {
                let destination = ThreadBuilder::default().build(std::thread::current().id());
                relocated.relocate(None, &destination);
                let destination_driver = relocated
                    .driver(DriverOptions::new(destination, Waker::noop().clone(), DriverRole::Primary))
                    .unwrap();
                let source = recording_source();
                let (thread_tx, thread_rx) = mpsc::channel();
                let registration = relocated.register(source.with_reporter(thread_tx), Interest::READABLE).unwrap();
                assert_eq!(thread_rx.recv().unwrap(), std::thread::current().id());
                drop(registration);
                destination_driver.shutdown().unwrap();
            })
            .join()
            .unwrap();
    });

    io.relocate(None, &ThreadBuilder::default().build(std::thread::current().id()));
    driver.shutdown().unwrap();
}

#[test]
fn unattached_context_rejects_registration_and_releases_the_source() {
    let io = MioContext::context(MioOptions::default());
    let (token_tx, _token_rx) = mpsc::channel();
    let (drop_tx, drop_rx) = mpsc::channel();

    let error = io
        .register(FailingRegisterSource { token_tx, drop_tx }, Interest::READABLE)
        .err()
        .unwrap();

    assert_eq!(error.kind(), io::ErrorKind::NotConnected);
    assert_eq!(drop_rx.recv().unwrap(), std::thread::current().id());
}

#[test]
fn relocation_keeps_existing_sources_on_their_original_poller() {
    let (io, mut driver) = driver(DriverRole::Primary, Waker::noop().clone());
    let (old_source, old_sender) = udp_pair();
    let old_registration = io.register(old_source, Interest::READABLE).unwrap();
    assert!(poll_read_ready(&old_registration, Waker::noop()).is_pending());
    let (destination_ready_tx, destination_ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    std::thread::scope(|scope| {
        let mut relocated = io.clone();
        let destination = scope.spawn(move || {
            let thread = ThreadBuilder::default().build(std::thread::current().id());
            relocated.relocate(None, &thread);
            let destination_driver = relocated
                .driver(DriverOptions::new(thread, Waker::noop().clone(), DriverRole::Primary))
                .unwrap();
            let source = recording_source();
            let (thread_tx, thread_rx) = mpsc::channel();
            let registration = relocated.register(source.with_reporter(thread_tx), Interest::READABLE).unwrap();
            assert_eq!(thread_rx.recv().unwrap(), std::thread::current().id());
            destination_ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(registration);
            destination_driver.shutdown().unwrap();
        });

        destination_ready_rx.recv().unwrap();
        old_sender.send(&[1]).unwrap();
        driver.execute_cycle(&mut Cycle::new(Duration::from_secs(1))).unwrap();
        expect_ready(poll_read_ready(&old_registration, Waker::noop())).unwrap();
        release_tx.send(()).unwrap();
        destination.join().unwrap();
    });

    drop(old_registration);
    driver.shutdown().unwrap();
}

#[test]
fn driver_shutdown_closes_registration_without_taking_source_ownership() {
    for role in [DriverRole::Primary, DriverRole::Secondary] {
        let current = std::thread::current().id();
        let (io, driver) = driver(role, Waker::noop().clone());
        let (deregister_tx, deregister_rx) = mpsc::channel();
        let (drop_tx, drop_rx) = mpsc::channel();
        let registration = io
            .register(
                recording_source()
                    .with_deregister_reporter(deregister_tx)
                    .with_drop_reporter(drop_tx),
                Interest::READABLE,
            )
            .unwrap();

        driver.shutdown().unwrap();

        assert_eq!(registration.source().err().unwrap(), RegistrationClosed);
        assert_eq!(deregister_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(drop_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        drop(registration);
        assert_eq!(drop_rx.recv().unwrap(), current);
        assert_eq!(deregister_rx.recv(), Err(mpsc::RecvError));
    }
}

#[test]
fn shutdown_can_race_source_access_and_releases_on_the_owner_thread() {
    let current = std::thread::current().id();
    let (io, driver) = driver(DriverRole::Primary, Waker::noop().clone());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (drop_tx, drop_rx) = mpsc::channel();
    let registration = io
        .register(recording_source().with_drop_reporter(drop_tx), Interest::READABLE)
        .unwrap();

    let owner = std::thread::spawn(move || {
        let mut registration = registration;
        {
            let _source = registration.source_mut().unwrap();
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }
        assert_eq!(registration.source_mut().err().unwrap(), RegistrationClosed);
        drop(registration);
    });

    entered_rx.recv().unwrap();
    driver.shutdown().unwrap();
    release_tx.send(()).unwrap();
    owner.join().unwrap();

    assert_ne!(drop_rx.recv().unwrap(), current);
}

#[test]
fn drop_deregistration_failure_is_reported_by_each_driver_role() {
    let (primary_io, mut primary_driver) = driver(DriverRole::Primary, Waker::noop().clone());
    let primary = primary_io.register(FailingDeregisterSource, Interest::READABLE).unwrap();
    drop(primary);

    let error = primary_driver.execute_cycle(&mut Cycle::new(Duration::ZERO)).unwrap_err();
    assert_eq!(error.to_string(), "scripted deregistration failure");
    primary_driver.shutdown().unwrap();

    let (owner_tx, owner_rx) = mpsc::channel();
    let (secondary_io, mut secondary_driver) = driver(DriverRole::Secondary, Waker::from(Arc::new(ChannelWake(owner_tx))));
    let secondary = secondary_io.register(FailingDeregisterSource, Interest::READABLE).unwrap();
    while owner_rx.try_recv().is_ok() {}
    drop(secondary);
    owner_rx.recv().unwrap();

    let error = secondary_driver.execute_cycle(&mut Cycle::new(Duration::ZERO)).unwrap_err();
    assert_eq!(error.to_string(), "scripted deregistration failure");
    secondary_driver.shutdown().unwrap();
}

#[test]
fn successful_registry_operations_do_not_wake_the_driver_owner() {
    for role in [DriverRole::Primary, DriverRole::Secondary] {
        let owner = Arc::new(WakeCounter::default());
        let (io, driver) = driver(role, Waker::from(Arc::clone(&owner)));
        let registration = io.register(recording_source(), Interest::READABLE).unwrap();
        assert_eq!(owner.count(), 0);
        drop(registration);
        assert_eq!(owner.count(), 0);
        driver.shutdown().unwrap();
    }
}

#[test]
fn failed_registration_releases_the_source_and_does_not_reuse_its_token() {
    for role in [DriverRole::Primary, DriverRole::Secondary] {
        let (io, driver) = driver(role, Waker::noop().clone());
        let (failed_token_tx, failed_token_rx) = mpsc::channel();
        let (drop_tx, drop_rx) = mpsc::channel();
        let error = io
            .register(
                FailingRegisterSource {
                    token_tx: failed_token_tx,
                    drop_tx,
                },
                Interest::READABLE,
            )
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "scripted registration failure");
        let failed_token = failed_token_rx.recv().unwrap();
        assert_eq!(drop_rx.recv().unwrap(), std::thread::current().id());

        let (next_token_tx, next_token_rx) = mpsc::channel();
        let registration = io.register(TokenReportingSource(next_token_tx), Interest::READABLE).unwrap();
        assert_eq!(next_token_rx.recv().unwrap().0, failed_token.0 + 1);

        drop(registration);
        driver.shutdown().unwrap();
    }
}

#[test]
fn driver_waker_is_latched_distinct_coalescing_and_inert_after_drop() {
    let owner_waker = Waker::from(Arc::new(WakeCounter::default()));
    let (_io, mut driver) = driver(DriverRole::Primary, owner_waker.clone());
    let driver_waker = driver.waker();

    assert!(!driver_waker.will_wake(&owner_waker));
    driver_waker.wake_by_ref();
    driver_waker.wake_by_ref();
    driver.execute_cycle(&mut Cycle::new(Duration::MAX)).unwrap();

    driver.shutdown().unwrap();
    driver_waker.wake_by_ref();
}

#[test]
fn shutdown_closes_live_registration_and_rejects_later_registration() {
    let (io, driver) = driver(DriverRole::Primary, Waker::noop().clone());
    let mut live = io.register(recording_source(), Interest::READABLE).unwrap();
    let live_wake = Arc::new(WakeCounter::default());
    assert!(poll_read_ready(&live, &Waker::from(Arc::clone(&live_wake))).is_pending());

    driver.shutdown().unwrap();

    assert_eq!(live.source_mut().err().unwrap(), RegistrationClosed);
    assert_eq!(live_wake.count(), 1);
    assert_eq!(expect_ready(poll_read_ready(&live, Waker::noop())), Err(RegistrationClosed));
    let later_source = recording_source();
    let error = io.register(later_source, Interest::READABLE).err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
}

#[test]
fn shutdown_reports_accumulated_readiness_before_closure() {
    let (io, mut driver) = driver(DriverRole::Primary, Waker::noop().clone());
    let (receiver, sender) = udp_pair();
    let registration = io.register(receiver, Interest::READABLE).unwrap();

    sender.send(&[1]).unwrap();
    driver.execute_cycle(&mut Cycle::new(Duration::from_secs(1))).unwrap();
    driver.shutdown().unwrap();

    assert_eq!(expect_ready(poll_read_ready(&registration, Waker::noop())), Ok(()));
    assert_eq!(expect_ready(poll_read_ready(&registration, Waker::noop())), Err(RegistrationClosed));
}

fn driver(role: DriverRole, owner_waker: Waker) -> (MioContext, MioDriver) {
    let mut io = MioContext::context(MioOptions::default());
    let worker = ThreadBuilder::default().build(std::thread::current().id());
    io.relocate(None, &worker);
    let driver = io.driver(DriverOptions::new(worker, owner_waker, role)).unwrap();
    (io, driver)
}

fn udp_pair() -> (UdpSocket, StdUdpSocket) {
    let receiver = UdpSocket::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let sender = StdUdpSocket::bind("127.0.0.1:0").unwrap();
    sender.connect(receiver.local_addr().unwrap()).unwrap();
    (receiver, sender)
}

fn tcp_pair() -> (TcpStream, StdTcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let peer = StdTcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (stream, _) = listener.accept().unwrap();
    stream.set_nonblocking(true).unwrap();
    (TcpStream::from_std(stream), peer)
}

fn recording_source() -> RecordingSource {
    RecordingSource {
        reporter: None,
        deregister_reporter: None,
        drop_reporter: None,
    }
}

fn poll_read_ready<S>(registration: &Registered<S>, waker: &Waker) -> Poll<Result<(), RegistrationClosed>>
where
    S: Source + Send + 'static,
{
    registration.poll_read_ready(&mut Context::from_waker(waker))
}

fn poll_write_ready<S>(registration: &Registered<S>, waker: &Waker) -> Poll<Result<(), RegistrationClosed>>
where
    S: Source + Send + 'static,
{
    registration.poll_write_ready(&mut Context::from_waker(waker))
}

fn expect_ready<T>(poll: Poll<T>) -> T {
    match poll {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("the future must be ready"),
    }
}

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl WakeCounter {
    fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

struct ChannelWake(mpsc::Sender<()>);

impl Wake for ChannelWake {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send(());
    }
}

struct RecordingSource {
    reporter: Option<mpsc::Sender<ThreadId>>,
    deregister_reporter: Option<mpsc::Sender<ThreadId>>,
    drop_reporter: Option<mpsc::Sender<ThreadId>>,
}

impl RecordingSource {
    fn with_reporter(mut self, reporter: mpsc::Sender<ThreadId>) -> Self {
        self.reporter = Some(reporter);
        self
    }

    fn with_deregister_reporter(mut self, reporter: mpsc::Sender<ThreadId>) -> Self {
        self.deregister_reporter = Some(reporter);
        self
    }

    fn with_drop_reporter(mut self, reporter: mpsc::Sender<ThreadId>) -> Self {
        self.drop_reporter = Some(reporter);
        self
    }
}

impl Drop for RecordingSource {
    fn drop(&mut self) {
        if let Some(reporter) = &self.drop_reporter {
            let _ = reporter.send(std::thread::current().id());
        }
    }
}

impl Source for RecordingSource {
    fn register(&mut self, registry: &Registry, token: Token, interests: Interest) -> io::Result<()> {
        if let Some(reporter) = &self.reporter {
            let _ = reporter.send(std::thread::current().id());
        }
        let _ = (registry, token, interests);
        Ok(())
    }

    fn reregister(&mut self, registry: &Registry, token: Token, interests: Interest) -> io::Result<()> {
        let _ = (registry, token, interests);
        Ok(())
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        if let Some(reporter) = &self.deregister_reporter {
            let _ = reporter.send(std::thread::current().id());
        }
        let _ = registry;
        Ok(())
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

struct FailingRegisterSource {
    token_tx: mpsc::Sender<Token>,
    drop_tx: mpsc::Sender<ThreadId>,
}

impl Drop for FailingRegisterSource {
    fn drop(&mut self) {
        let _ = self.drop_tx.send(std::thread::current().id());
    }
}

impl Source for FailingRegisterSource {
    fn register(&mut self, _registry: &Registry, token: Token, _interests: Interest) -> io::Result<()> {
        self.token_tx.send(token).unwrap();
        Err(io::Error::other("scripted registration failure"))
    }

    fn reregister(&mut self, _registry: &Registry, _token: Token, _interests: Interest) -> io::Result<()> {
        unreachable!("the test source never registers")
    }

    fn deregister(&mut self, _registry: &Registry) -> io::Result<()> {
        unreachable!("the test source never registers")
    }
}

struct TokenReportingSource(mpsc::Sender<Token>);

impl Source for TokenReportingSource {
    fn register(&mut self, _registry: &Registry, token: Token, _interests: Interest) -> io::Result<()> {
        self.0.send(token).unwrap();
        Ok(())
    }

    fn reregister(&mut self, _registry: &Registry, _token: Token, _interests: Interest) -> io::Result<()> {
        Ok(())
    }

    fn deregister(&mut self, _registry: &Registry) -> io::Result<()> {
        Ok(())
    }
}
