// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::poll_fn;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use arty::task::Scheduler;
use arty_mio::{MioContext, Registered};
use fetch_options::SocketOptions;
use http::uri::Scheme;
use http_extensions::{HttpError, Result};
use layered::Service;
use mio::Interest;
use mio::net::TcpStream;
use seatbelt::RecoveryInfo;
use socket2::SockRef;
use templated_uri::BaseUri;
use tick::Clock;

use crate::MioStream;
use crate::error::{
    AttemptFailure, AttemptPhase, LABEL_DNS, LABEL_RUNTIME, LABEL_SCHEME_NOT_ALLOWED, connect_failures, io_error, unavailable,
};
use crate::stream::registration_closed;

const HAPPY_EYEBALLS_DELAY: Duration = Duration::from_millis(300);

trait Resolver {
    fn resolve(&self, host: String, port: u16) -> impl Future<Output = Result<Vec<SocketAddr>>> + Send + 'static;
}

trait Dialer {
    type Stream;
    type Future: Future<Output = std::result::Result<Self::Stream, AttemptFailure>> + Send + Unpin + 'static;

    fn connect(&self, address: SocketAddr) -> Self::Future;
}

/// Configuration for [`MioConnector`].
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct MioConnectorOptions {
    /// Socket settings for every outbound TCP connection.
    pub socket: SocketOptions,
}

impl MioConnectorOptions {
    /// Sets the socket settings for every outbound TCP connection.
    #[must_use]
    pub fn socket(mut self, socket: SocketOptions) -> Self {
        self.socket = socket;
        self
    }
}

/// A cloneable service that opens Arty Mio TCP streams for Hyper.
///
/// Create this value on its target Arty worker. Its [`MioContext`] routes each
/// new stream registration to that worker. Clones keep the same worker route.
/// Fetch creates one connector for each worker.
#[derive(Clone, Debug)]
pub struct MioConnector {
    context: MioContext,
    scheduler: Scheduler,
    clock: Clock,
    options: MioConnectorOptions,
}

impl MioConnector {
    /// Creates a connector for one Arty worker.
    #[must_use]
    pub const fn new(context: MioContext, scheduler: Scheduler, clock: Clock, options: MioConnectorOptions) -> Self {
        Self {
            context,
            scheduler,
            clock,
            options,
        }
    }
}

impl Service<BaseUri> for MioConnector {
    type Out = Result<MioStream>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        validate_scheme(&input)?;
        let port = input.try_effective_port()?;
        connect_with(
            &self.clock,
            input.host().to_owned(),
            port,
            &SystemResolver {
                scheduler: self.scheduler.clone(),
            },
            &MioDialer {
                context: self.context.clone(),
                options: self.options.socket,
            },
        )
        .await
    }
}

fn validate_scheme(input: &BaseUri) -> Result<()> {
    let scheme = input.scheme();
    if scheme == &Scheme::HTTP || scheme == &Scheme::HTTPS {
        return Ok(());
    }

    Err(HttpError::other(
        format!("the connector does not support the '{scheme}' scheme; only http and https are supported"),
        RecoveryInfo::never(),
        LABEL_SCHEME_NOT_ALLOWED,
    ))
}

async fn connect_with<R, D>(clock: &Clock, host: String, port: u16, resolver: &R, dialer: &D) -> Result<D::Stream>
where
    R: Resolver + Sync,
    D: Dialer + Sync,
    D::Stream: Send,
{
    let addresses = resolver.resolve(host, port).await?;
    race_connections(clock, addresses, dialer).await
}

#[derive(Clone)]
struct SystemResolver {
    scheduler: Scheduler,
}

impl Resolver for SystemResolver {
    fn resolve(&self, host: String, port: u16) -> impl Future<Output = Result<Vec<SocketAddr>>> + Send + 'static {
        let scheduler = self.scheduler.clone();
        async move {
            let result = scheduler
                .spawn_blocking(move || {
                    let addresses = (host.as_str(), port).to_socket_addrs()?.collect::<Vec<_>>();
                    if addresses.is_empty() {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrNotAvailable,
                            "DNS resolution returned no socket addresses",
                        ));
                    }
                    Ok(order_addresses(addresses))
                })
                .await
                .map_err(|error| unavailable(error, LABEL_RUNTIME))?;

            result.map_err(|error| io_error(error, LABEL_DNS))
        }
    }
}

fn order_addresses(addresses: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let mut unique = Vec::with_capacity(addresses.len());
    for address in addresses {
        if !unique.contains(&address) {
            unique.push(address);
        }
    }

    let prefer_ipv6 = unique.first().is_some_and(SocketAddr::is_ipv6);
    let mut ipv4 = unique.iter().copied().filter(SocketAddr::is_ipv4);
    let mut ipv6 = unique.iter().copied().filter(SocketAddr::is_ipv6);
    let mut ordered = Vec::with_capacity(unique.len());

    loop {
        let first = if prefer_ipv6 { ipv6.next() } else { ipv4.next() };
        let second = if prefer_ipv6 { ipv4.next() } else { ipv6.next() };
        if first.is_none() && second.is_none() {
            break;
        }
        ordered.extend(first);
        ordered.extend(second);
    }

    ordered
}

async fn race_connections<D>(clock: &Clock, addresses: Vec<SocketAddr>, dialer: &D) -> Result<D::Stream>
where
    D: Dialer + Sync,
    D::Stream: Send,
{
    if addresses.is_empty() {
        return Err(io_error(
            io::Error::new(io::ErrorKind::AddrNotAvailable, "DNS resolution returned no socket addresses"),
            LABEL_DNS,
        ));
    }

    let mut attempts = Attempts::new(addresses.len());
    let mut failures = std::iter::repeat_with(|| None)
        .take(addresses.len())
        .collect::<Vec<Option<AttemptFailure>>>();
    let mut next = 0;

    launch(&mut attempts, next, addresses[next], dialer);
    next += 1;
    let mut delay = clock.delay(HAPPY_EYEBALLS_DELAY);

    loop {
        if next == addresses.len() {
            let Some((index, result)) = attempts.next().await else {
                return Err(finish_failures(failures));
            };
            match result {
                Ok(stream) => return Ok(stream),
                Err(error) => failures[index] = Some(error),
            }
            continue;
        }

        let attempt = poll_fn(|cx| {
            if let Poll::Ready(attempt) = attempts.poll_next(cx) {
                return Poll::Ready(attempt);
            }
            if Pin::new(&mut delay).poll(cx).is_ready() {
                return Poll::Ready(None);
            }
            Poll::Pending
        })
        .await;

        if let Some((index, result)) = attempt {
            match result {
                Ok(stream) => return Ok(stream),
                Err(error) => {
                    failures[index] = Some(error);
                    launch(&mut attempts, next, addresses[next], dialer);
                    next += 1;
                    delay = clock.delay(HAPPY_EYEBALLS_DELAY);
                }
            }
        } else {
            launch(&mut attempts, next, addresses[next], dialer);
            next += 1;
            delay = clock.delay(HAPPY_EYEBALLS_DELAY);
        }
    }
}

fn launch<D>(attempts: &mut Attempts<D::Stream, D::Future>, index: usize, address: SocketAddr, dialer: &D)
where
    D: Dialer,
{
    attempts.insert(index, dialer.connect(address));
}

#[derive(Clone)]
struct MioDialer {
    context: MioContext,
    options: SocketOptions,
}

impl Dialer for MioDialer {
    type Stream = MioStream;
    type Future = MioConnectFuture;

    fn connect(&self, address: SocketAddr) -> Self::Future {
        MioConnectFuture {
            address,
            options: self.options,
            state: MioConnectState::Start(self.context.clone()),
        }
    }
}

struct MioConnectFuture {
    address: SocketAddr,
    options: SocketOptions,
    state: MioConnectState,
}

enum MioConnectState {
    Start(MioContext),
    Connecting(Registered<TcpStream>),
    Done,
}

impl Future for MioConnectFuture {
    type Output = std::result::Result<MioStream, AttemptFailure>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if matches!(this.state, MioConnectState::Start(_)) {
            let MioConnectState::Start(context) = std::mem::replace(&mut this.state, MioConnectState::Done) else {
                return Poll::Ready(Err(completed_dial_error(this.address)));
            };
            let stream = match TcpStream::connect(this.address) {
                Ok(stream) => stream,
                Err(error) => return Poll::Ready(Err(AttemptFailure::new(this.address, AttemptPhase::Connect, error))),
            };
            if let Err(error) = apply_buffer_options(&stream, this.options) {
                return Poll::Ready(Err(AttemptFailure::new(this.address, AttemptPhase::SocketOption, error)));
            }
            let registration = match context.register(stream, Interest::READABLE | Interest::WRITABLE) {
                Ok(registration) => registration,
                Err(error) => return Poll::Ready(Err(AttemptFailure::new(this.address, AttemptPhase::Registration, error))),
            };
            this.state = MioConnectState::Connecting(registration);
        }

        let MioConnectState::Connecting(registration) = &this.state else {
            return Poll::Ready(Err(completed_dial_error(this.address)));
        };
        match poll_complete_connect(registration, cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(error)) => {
                this.state = MioConnectState::Done;
                return Poll::Ready(Err(AttemptFailure::new(this.address, AttemptPhase::Connect, error)));
            }
            Poll::Pending => return Poll::Pending,
        }

        if let Some(no_delay) = this.options.no_delay
            && let Err(error) = registration
                .source()
                .map_err(registration_closed)
                .and_then(|source| source.set_nodelay(no_delay))
        {
            this.state = MioConnectState::Done;
            return Poll::Ready(Err(AttemptFailure::new(this.address, AttemptPhase::SocketOption, error)));
        }

        let MioConnectState::Connecting(registration) = std::mem::replace(&mut this.state, MioConnectState::Done) else {
            return Poll::Ready(Err(completed_dial_error(this.address)));
        };
        Poll::Ready(Ok(MioStream::new(registration)))
    }
}

fn completed_dial_error(address: SocketAddr) -> AttemptFailure {
    AttemptFailure::new(
        address,
        AttemptPhase::Connect,
        io::Error::other("the completed dial future was polled again"),
    )
}

fn apply_buffer_options(stream: &TcpStream, options: SocketOptions) -> io::Result<()> {
    let socket = SockRef::from(stream);
    if let Some(size) = options.receive_buffer_size {
        socket.set_recv_buffer_size(size as usize)?;
    }
    if let Some(size) = options.send_buffer_size {
        socket.set_send_buffer_size(size as usize)?;
    }
    Ok(())
}

fn poll_complete_connect(registration: &Registered<TcpStream>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
    loop {
        match registration.poll_write_ready(context) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(error)) => return Poll::Ready(Err(registration_closed(error))),
            Poll::Pending => return Poll::Pending,
        }

        loop {
            let source = match registration.source() {
                Ok(source) => source,
                Err(error) => return Poll::Ready(Err(registration_closed(error))),
            };
            let connect_error = match source.take_error() {
                Ok(None) => None,
                Ok(Some(error)) | Err(error) => Some(error),
            };
            if let Some(error) = connect_error {
                match connect_error_action(&error) {
                    ConnectErrorAction::Retry => continue,
                    ConnectErrorAction::Wait => break,
                    ConnectErrorAction::Fail => return Poll::Ready(Err(error)),
                }
            }

            match source.peer_addr() {
                Ok(_) => return Poll::Ready(Ok(())),
                Err(error) => match connect_error_action(&error) {
                    ConnectErrorAction::Retry => {}
                    ConnectErrorAction::Wait => break,
                    ConnectErrorAction::Fail => return Poll::Ready(Err(error)),
                },
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ConnectErrorAction {
    Retry,
    Wait,
    Fail,
}

fn connect_error_action(error: &io::Error) -> ConnectErrorAction {
    if error.kind() == io::ErrorKind::Interrupted {
        ConnectErrorAction::Retry
    } else if is_connect_pending(error) {
        ConnectErrorAction::Wait
    } else {
        ConnectErrorAction::Fail
    }
}

fn is_connect_pending(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::NotConnected) || is_in_progress(error)
}

#[cfg(unix)]
fn is_in_progress(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::EINPROGRESS)
}

#[cfg(windows)]
fn is_in_progress(error: &io::Error) -> bool {
    const WSAEINPROGRESS: i32 = 10_036;
    error.raw_os_error() == Some(WSAEINPROGRESS)
}

#[cfg(not(any(unix, windows)))]
const fn is_in_progress(_error: &io::Error) -> bool {
    false
}

fn finish_failures(failures: Vec<Option<AttemptFailure>>) -> HttpError {
    connect_failures(failures.into_iter().flatten().collect())
}

struct Attempts<T, F> {
    slots: Vec<Option<F>>,
    active: usize,
    _stream: std::marker::PhantomData<fn() -> T>,
}

impl<T, F> Attempts<T, F>
where
    F: Future<Output = std::result::Result<T, AttemptFailure>> + Unpin,
{
    fn new(capacity: usize) -> Self {
        Self {
            slots: std::iter::repeat_with(|| None).take(capacity).collect(),
            active: 0,
            _stream: std::marker::PhantomData,
        }
    }

    fn insert(&mut self, index: usize, future: F) {
        self.slots[index] = Some(future);
        self.active += 1;
    }

    async fn next(&mut self) -> Option<(usize, std::result::Result<T, AttemptFailure>)> {
        poll_fn(|cx| self.poll_next(cx)).await
    }

    fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<(usize, std::result::Result<T, AttemptFailure>)>> {
        if self.active == 0 {
            return Poll::Ready(None);
        }

        for (index, slot) in self.slots.iter_mut().enumerate() {
            let Some(future) = slot else {
                continue;
            };
            if let Poll::Ready(result) = Pin::new(future).poll(cx) {
                *slot = None;
                self.active -= 1;
                return Poll::Ready(Some((index, result)));
            }
        }

        Poll::Pending
    }
}

#[cfg(feature = "benchmark")]
#[doc(hidden)]
pub mod benchmark {
    use std::net::SocketAddr;
    use std::pin::{Pin, pin};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;
    use std::{fmt, io};

    use http_extensions::Result;
    use tick::{Clock, ClockControl};

    use super::{AttemptFailure, AttemptPhase, Dialer, Resolver, connect_with, order_addresses};

    /// A deterministic connector path for benchmarks.
    #[derive(Clone, Copy, Debug)]
    pub enum Scenario {
        /// One address succeeds.
        OneAddress,
        /// The first of two addresses succeeds.
        TwoAddressesFirstSuccess,
        /// The second address starts after the 300 ms delay.
        DelayedSecondLaunch,
        /// The first address fails and the second address succeeds.
        FastFailureThenSuccess,
        /// Both addresses fail.
        AllFailures,
    }

    impl Scenario {
        const fn scripts(self) -> &'static [AttemptScript] {
            match self {
                Self::OneAddress | Self::TwoAddressesFirstSuccess => &[AttemptScript::Success],
                Self::DelayedSecondLaunch => &[AttemptScript::Pending, AttemptScript::Success],
                Self::FastFailureThenSuccess => &[AttemptScript::Failure, AttemptScript::Success],
                Self::AllFailures => &[AttemptScript::Failure, AttemptScript::Failure],
            }
        }

        fn addresses(self) -> Vec<SocketAddr> {
            let mut addresses = vec![SocketAddr::from(([192, 0, 2, 1], 443))];
            if !matches!(self, Self::OneAddress) {
                addresses.push(SocketAddr::from(([0x2001, 0xdb8, 0, 0, 0, 0, 0, 1], 443)));
            }
            addresses
        }

        const fn needs_delay_advance(self) -> bool {
            matches!(self, Self::DelayedSecondLaunch)
        }
    }

    /// Prepared state for one connector benchmark operation.
    pub struct Harness {
        scenario: Scenario,
        control: ClockControl,
        clock: Clock,
        resolver: ScriptedResolver,
        dialer: ScriptedDialer,
    }

    impl fmt::Debug for Harness {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Harness").field("scenario", &self.scenario).finish_non_exhaustive()
        }
    }

    impl Harness {
        /// Prepares one benchmark operation.
        #[must_use]
        pub fn new(scenario: Scenario) -> Self {
            let control = ClockControl::new();
            Self {
                scenario,
                clock: control.to_clock(),
                control,
                resolver: ScriptedResolver { scenario },
                dialer: ScriptedDialer {
                    scripts: scenario.scripts(),
                    launches: AtomicUsize::new(0),
                },
            }
        }
    }

    /// The retained output from one connector benchmark operation.
    pub struct Output {
        harness: Harness,
        result: Result<usize>,
        launches: usize,
    }

    impl fmt::Debug for Output {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Output")
                .field("harness", &self.harness)
                .field("result", &self.result)
                .field("launches", &self.launches)
                .finish()
        }
    }

    impl Output {
        /// Returns the number of address attempts that started.
        #[must_use]
        pub const fn launches(&self) -> usize {
            self.launches
        }

        /// Returns true when the connector succeeded.
        #[must_use]
        pub const fn succeeded(&self) -> bool {
            self.result.is_ok()
        }

        /// Keeps the prepared state alive until after measurement.
        pub fn retain(self) {
            std::hint::black_box(self.harness);
            let _ = std::hint::black_box(self.result);
        }
    }

    /// Runs one deterministic connector benchmark operation.
    #[must_use]
    #[expect(clippy::panic, reason = "benchmark scripts fail fast when production behavior changes")]
    pub fn run(harness: Harness) -> Output {
        let result = {
            let future = connect_with(
                &harness.clock,
                "benchmark.invalid".to_owned(),
                443,
                &harness.resolver,
                &harness.dialer,
            );
            // Stack-pin to keep benchmark allocations limited to the connector.
            let mut future = pin!(future);
            match poll(future.as_mut()) {
                Poll::Ready(output) => output,
                Poll::Pending if harness.scenario.needs_delay_advance() => {
                    harness.control.advance(Duration::from_millis(300));
                    expect_ready(poll(future.as_mut()))
                }
                Poll::Pending => panic!("the scripted connector did not complete"),
            }
        };

        let launches = harness.dialer.launches.load(Ordering::Relaxed);
        Output { harness, result, launches }
    }

    fn poll<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    #[expect(clippy::panic, reason = "benchmark scripts fail fast when production behavior changes")]
    fn expect_ready<T>(poll: Poll<T>) -> T {
        match poll {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("the scripted connector did not complete after the timer advanced"),
        }
    }

    struct ScriptedResolver {
        scenario: Scenario,
    }

    impl Resolver for ScriptedResolver {
        fn resolve(&self, _host: String, _port: u16) -> impl Future<Output = Result<Vec<SocketAddr>>> + Send + 'static {
            let scenario = self.scenario;
            async move { Ok(order_addresses(scenario.addresses())) }
        }
    }

    #[derive(Clone, Copy)]
    enum AttemptScript {
        Pending,
        Success,
        Failure,
    }

    struct ScriptedDialer {
        scripts: &'static [AttemptScript],
        launches: AtomicUsize,
    }

    impl Dialer for ScriptedDialer {
        type Stream = usize;
        type Future = ScriptedAttempt;

        fn connect(&self, address: SocketAddr) -> Self::Future {
            let index = self.launches.fetch_add(1, Ordering::Relaxed);
            ScriptedAttempt {
                address,
                index,
                script: self.scripts[index],
            }
        }
    }

    struct ScriptedAttempt {
        address: SocketAddr,
        index: usize,
        script: AttemptScript,
    }

    impl Future for ScriptedAttempt {
        type Output = std::result::Result<usize, AttemptFailure>;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            match self.script {
                AttemptScript::Pending => Poll::Pending,
                AttemptScript::Success => Poll::Ready(Ok(self.index)),
                AttemptScript::Failure => Poll::Ready(Err(AttemptFailure::new(
                    self.address,
                    AttemptPhase::Connect,
                    io::Error::from(io::ErrorKind::ConnectionRefused),
                ))),
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::task::Waker;

    use future_assert::FutureTestExt as _;
    use ohno::Labeled as _;
    use seatbelt::{Recovery as _, RecoveryKind};
    use tick::ClockControl;

    use super::*;

    #[test]
    fn address_order_interleaves_families_and_removes_duplicates() {
        let v4_a = "127.0.0.1:80".parse().unwrap();
        let v4_b = "127.0.0.2:80".parse().unwrap();
        let v6_a = "[::1]:80".parse().unwrap();
        let v6_b = "[::2]:80".parse().unwrap();

        assert_eq!(order_addresses(vec![v6_a, v6_b, v4_a, v4_b, v6_a]), vec![v6_a, v4_a, v6_b, v4_b]);
        assert_eq!(order_addresses(vec![v4_a, v4_b, v6_a, v6_b, v4_a]), vec![v4_a, v6_a, v4_b, v6_b]);
    }

    #[test]
    fn connect_pending_recognizes_platform_in_progress() {
        #[cfg(unix)]
        let error = io::Error::from_raw_os_error(libc::EINPROGRESS);
        #[cfg(windows)]
        let error = io::Error::from_raw_os_error(10_036);
        #[cfg(not(any(unix, windows)))]
        let error = io::Error::from(io::ErrorKind::WouldBlock);

        assert_eq!(connect_error_action(&error), ConnectErrorAction::Wait);
        assert_eq!(
            connect_error_action(&io::Error::from(io::ErrorKind::NotConnected)),
            ConnectErrorAction::Wait
        );
        assert_eq!(
            connect_error_action(&io::Error::from(io::ErrorKind::Interrupted)),
            ConnectErrorAction::Retry
        );
        assert_eq!(
            connect_error_action(&io::Error::from(io::ErrorKind::ConnectionRefused)),
            ConnectErrorAction::Fail
        );
    }

    #[test]
    fn resolver_seam_returns_scripted_addresses_without_dns() {
        let expected = vec!["192.0.2.1:443".parse().unwrap(), "[2001:db8::1]:443".parse().unwrap()];
        let resolver = ScriptedResolver::success(expected.clone());

        assert_eq!(resolver.resolve("unused.invalid".to_owned(), 443).unwrap_ready().unwrap(), expected);
        assert_eq!(resolver.calls(), vec![("unused.invalid".to_owned(), 443)]);
    }

    #[test]
    fn happy_eyeballs_launches_second_address_after_300_ms() {
        let first = ManualAttempt::new(AttemptPhase::Connect);
        let dialer = ScriptedDialer::new(vec![AttemptScript::Pending(first.future()), AttemptScript::Success("second")]);
        let control = ClockControl::new();
        let clock = control.to_clock();
        let addresses = addresses();
        let mut future = std::pin::pin!(race_connections(&clock, addresses.clone(), &dialer));

        assert!(poll(future.as_mut()).is_pending());
        assert_eq!(dialer.launches(), vec![addresses[0]]);

        control.advance(Duration::from_millis(299));
        assert!(poll(future.as_mut()).is_pending());
        assert_eq!(dialer.launches(), vec![addresses[0]]);

        control.advance(Duration::from_millis(1));
        assert_eq!(poll(future.as_mut()).unwrap_ready().unwrap(), "second");
        assert_eq!(dialer.launches(), addresses);
        assert_eq!(first.dropped(), 1, "the losing attempt must be cancelled");
    }

    #[test]
    fn fast_failure_launches_next_address_without_waiting() {
        let dialer = ScriptedDialer::new(vec![
            AttemptScript::Failure(AttemptPhase::Connect, io::ErrorKind::ConnectionRefused),
            AttemptScript::Success("second"),
        ]);
        let clock = ClockControl::new().to_clock();

        assert_eq!(race_connections(&clock, addresses(), &dialer).unwrap_ready().unwrap(), "second");
        assert_eq!(dialer.launches().len(), 2);
    }

    #[test]
    fn all_address_failures_keep_last_recovery_and_connect_label() {
        let dialer = ScriptedDialer::new(vec![
            AttemptScript::Failure(AttemptPhase::Registration, io::ErrorKind::BrokenPipe),
            AttemptScript::Failure(AttemptPhase::Connect, io::ErrorKind::NetworkUnreachable),
        ]);
        let clock = ClockControl::new().to_clock();
        let error = race_connections(&clock, addresses(), &dialer).unwrap_ready().unwrap_err();

        assert_eq!(error.label().as_str(), "connect");
        assert_eq!(error.recovery().kind(), RecoveryKind::Unavailable);
        assert!(error.to_string().contains("all 2 connection attempts failed"));
    }

    #[test]
    fn dropping_registration_and_connect_attempts_cleans_them_up() {
        for phase in [AttemptPhase::Registration, AttemptPhase::Connect] {
            let attempt = ManualAttempt::new(phase.clone());
            let dialer = ScriptedDialer::new(vec![AttemptScript::Pending(attempt.future())]);
            let clock = ClockControl::new().to_clock();
            let mut future = Box::pin(race_connections(&clock, vec![addresses()[0]], &dialer));

            assert!(poll(future.as_mut()).is_pending());
            drop(future);
            assert_eq!(attempt.dropped_phases(), vec![phase]);
        }
    }

    fn addresses() -> Vec<SocketAddr> {
        vec!["192.0.2.1:443".parse().unwrap(), "192.0.2.2:443".parse().unwrap()]
    }

    fn poll<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    trait PollReadyExt<T> {
        fn unwrap_ready(self) -> T;
    }

    impl<T> PollReadyExt<T> for Poll<T> {
        fn unwrap_ready(self) -> T {
            match self {
                Self::Ready(value) => value,
                Self::Pending => panic!("expected scripted future to be ready"),
            }
        }
    }

    #[derive(Clone)]
    struct ScriptedResolver {
        result: Arc<Mutex<Option<Result<Vec<SocketAddr>>>>>,
        calls: Arc<Mutex<Vec<(String, u16)>>>,
    }

    impl ScriptedResolver {
        fn success(addresses: Vec<SocketAddr>) -> Self {
            Self {
                result: Arc::new(Mutex::new(Some(Ok(addresses)))),
                calls: Arc::default(),
            }
        }

        fn calls(&self) -> Vec<(String, u16)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Resolver for ScriptedResolver {
        fn resolve(&self, host: String, port: u16) -> impl Future<Output = Result<Vec<SocketAddr>>> + Send + 'static {
            self.calls.lock().unwrap().push((host, port));
            let result = self.result.lock().unwrap().take().unwrap();
            std::future::ready(result)
        }
    }

    enum AttemptScript {
        Pending(ManualAttemptFuture),
        Success(&'static str),
        Failure(AttemptPhase, io::ErrorKind),
    }

    #[derive(Clone)]
    struct ScriptedDialer {
        scripts: Arc<Mutex<VecDeque<AttemptScript>>>,
        launches: Arc<Mutex<Vec<SocketAddr>>>,
    }

    impl ScriptedDialer {
        fn new(scripts: Vec<AttemptScript>) -> Self {
            Self {
                scripts: Arc::new(Mutex::new(scripts.into())),
                launches: Arc::default(),
            }
        }

        fn launches(&self) -> Vec<SocketAddr> {
            self.launches.lock().unwrap().clone()
        }
    }

    impl Dialer for ScriptedDialer {
        type Stream = &'static str;
        type Future = ScriptedAttemptFuture;

        fn connect(&self, address: SocketAddr) -> Self::Future {
            self.launches.lock().unwrap().push(address);
            let script = self.scripts.lock().unwrap().pop_front().unwrap();
            match script {
                AttemptScript::Pending(future) => ScriptedAttemptFuture::Pending(future),
                AttemptScript::Success(value) => ScriptedAttemptFuture::Ready(Some(Ok(value))),
                AttemptScript::Failure(phase, kind) => {
                    ScriptedAttemptFuture::Ready(Some(Err(AttemptFailure::new(address, phase, io::Error::from(kind)))))
                }
            }
        }
    }

    enum ScriptedAttemptFuture {
        Pending(ManualAttemptFuture),
        Ready(Option<std::result::Result<&'static str, AttemptFailure>>),
    }

    impl Future for ScriptedAttemptFuture {
        type Output = std::result::Result<&'static str, AttemptFailure>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            match &mut *self {
                Self::Pending(future) => Pin::new(future).poll(cx),
                Self::Ready(result) => Poll::Ready(result.take().unwrap()),
            }
        }
    }

    struct ManualAttempt {
        phase: AttemptPhase,
        dropped_phases: Arc<Mutex<Vec<AttemptPhase>>>,
    }

    impl ManualAttempt {
        fn new(phase: AttemptPhase) -> Self {
            Self {
                phase,
                dropped_phases: Arc::default(),
            }
        }

        fn future(&self) -> ManualAttemptFuture {
            ManualAttemptFuture {
                phase: self.phase.clone(),
                dropped_phases: Arc::clone(&self.dropped_phases),
            }
        }

        fn dropped(&self) -> usize {
            self.dropped_phases.lock().unwrap().len()
        }

        fn dropped_phases(&self) -> Vec<AttemptPhase> {
            self.dropped_phases.lock().unwrap().clone()
        }
    }

    struct ManualAttemptFuture {
        phase: AttemptPhase,
        dropped_phases: Arc<Mutex<Vec<AttemptPhase>>>,
    }

    impl Future for ManualAttemptFuture {
        type Output = std::result::Result<&'static str, AttemptFailure>;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    impl Drop for ManualAttemptFuture {
        fn drop(&mut self) {
            self.dropped_phases.lock().unwrap().push(self.phase.clone());
        }
    }
}
