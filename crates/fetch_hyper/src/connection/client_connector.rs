// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`ClientConnector`]: layered service producing a [`TrackedStream`] for
//! every successful connect, applying connect-timeout and HTTP-version
//! verification along the way.

use std::marker::PhantomData;
use std::time::Duration;

use fetch_options::{ConnectionInfo, ConnectionLifetime, PoolIndex};
use http::Version;
use http_extensions::{HttpError, Result};
use hyper_util::client::legacy::connect::{Connected, Connection};
use observed::{Sink, Text, emit};
use seatbelt::RecoveryInfo;
use templated_uri::BaseUri;
use tick::{Clock, FutureExt};

use crate::connection::connect::Connect;
use crate::connection::io::HyperIo;
use crate::connection::tracked_stream::TrackedStream;
use crate::error_labels::{LABEL_CONNECT, LABEL_HTTP_VERSION_UNSUPPORTED, collect_error_labels};
use crate::telemetry::{ConnectionError, ConnectionStart, ConnectionSuccess, protocol_version};

/// A connector service that applies connect-timeout, version verification,
/// and lifecycle tracking on top of a user-supplied [`Connect`].
pub(crate) struct ClientConnector<C, S> {
    connector: C,
    clock: Clock,
    connect_timeout: Duration,
    supported_versions: Vec<Version>,
    sink: Sink,
    pool_index: PoolIndex,
    connection_lifetime: ConnectionLifetime,
    _marker: PhantomData<fn() -> S>,
}

impl<C: Clone, S> Clone for ClientConnector<C, S> {
    fn clone(&self) -> Self {
        Self {
            connector: self.connector.clone(),
            clock: self.clock.clone(),
            connect_timeout: self.connect_timeout,
            supported_versions: self.supported_versions.clone(),
            sink: self.sink.clone(),
            pool_index: self.pool_index,
            connection_lifetime: self.connection_lifetime.clone(),
            _marker: PhantomData,
        }
    }
}

impl<C, S> ClientConnector<C, S> {
    pub(crate) fn new(
        connector: C,
        clock: Clock,
        connect_timeout: Duration,
        supported_versions: Vec<Version>,
        sink: Sink,
        pool_index: PoolIndex,
        connection_lifetime: ConnectionLifetime,
    ) -> Self {
        Self {
            connector,
            clock,
            connect_timeout,
            supported_versions,
            sink,
            pool_index,
            connection_lifetime,
            _marker: PhantomData,
        }
    }
}

impl<C, S> layered::Service<BaseUri> for ClientConnector<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    type Out = Result<TrackedStream<S>>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        let max_age = self.connection_lifetime.resolve();

        let connection = connect_with_timeout(
            self.connector.execute(input.clone()),
            input.clone(),
            &self.clock,
            self.connect_timeout,
            &self.sink,
        )
        .await?;

        let connected = connection.connected();
        verify_protocol_version(&connected, &input, &self.supported_versions)?;

        Ok(TrackedStream::new(
            connection,
            input,
            ConnectionInfo::new(connection_now(&self.clock), self.pool_index, max_age),
            self.sink.clone(),
            connected,
        ))
    }
}

/// Builds a `Fn() -> Instant` closure backed by `clock`, for handing to
/// [`ConnectionInfo::new`]. Cloning the [`Clock`] is cheap and lets the
/// closure outlive the borrow.
fn connection_now(clock: &Clock) -> impl Fn() -> std::time::Instant + Send + Sync + 'static {
    let clock = clock.clone();
    move || clock.instant()
}

async fn connect_with_timeout<S>(
    future: impl Future<Output = Result<S>>,
    base_uri: BaseUri,
    clock: &Clock,
    connect_timeout: Duration,
    sink: &Sink,
) -> Result<S>
where
    S: Connection,
{
    emit!(
        sink,
        ConnectionStart {
            server_address: base_uri.host(),
            server_port: base_uri.effective_port(),
            scheme: base_uri.scheme().as_str(),
            url_full: &base_uri,
        }
    );

    let stopwatch = clock.stopwatch();
    let result = future.timeout(clock, connect_timeout).await;
    let setup_duration_secs = stopwatch.elapsed().as_secs_f64();

    let error = match result {
        Ok(Ok(connection)) => {
            emit!(
                sink,
                ConnectionSuccess {
                    server_address: base_uri.host(),
                    server_port: base_uri.effective_port(),
                    scheme: base_uri.scheme().as_str(),
                    url_full: &base_uri,
                    protocol_version: protocol_version(&connection.connected()),
                    setup_duration_secs,
                }
            );

            return Ok(connection);
        }
        Ok(Err(error)) => error,
        Err(_timeout_error) => HttpError::other(
            format!(
                "server connection timeout, endpoint: {base_uri}, connection timeout(s): {}",
                connect_timeout.as_secs(),
            ),
            RecoveryInfo::retry(),
            LABEL_CONNECT,
        ),
    };

    emit!(
        sink,
        ConnectionError {
            server_address: base_uri.host(),
            server_port: base_uri.effective_port(),
            scheme: base_uri.scheme().as_str(),
            url_full: &base_uri,
            error_type: Text::from(collect_error_labels(&error).into_cow()),
            exception_message: Text::from(error.to_string()),
            setup_duration_secs,
        }
    );

    Err(error)
}

fn verify_protocol_version(info: &Connected, base_uri: &BaseUri, versions: &[Version]) -> Result<()> {
    if versions.len() == 1 && !base_uri.is_https() {
        return Ok(());
    }

    let negotiated = negotiated_version(info);

    if !versions.contains(&negotiated) {
        return Err(HttpError::other(
            format!(
                "the connection was established with unsupported HTTP version: {negotiated:?}, supported versions are: {versions:?}, server: {base_uri}"
            ),
            RecoveryInfo::never(),
            LABEL_HTTP_VERSION_UNSUPPORTED,
        ));
    }

    Ok(())
}

fn negotiated_version(connected: &Connected) -> Version {
    if connected.is_negotiated_h2() {
        Version::HTTP_2
    } else {
        Version::HTTP_11
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::future::{pending, ready};

    use bytes::Bytes;
    use layered::Service as _;
    use observed::Severity;
    use observed_testing::{CapturedEvent, ExpectedEvent, TEST_ID, test_emitter};
    use ohno::ErrorExt;

    use super::*;
    use crate::testing::{FakeConnector, FakeStream, TestError};

    #[test]
    fn negotiated_version_defaults_to_http_11() {
        assert_eq!(negotiated_version(&Connected::new()), Version::HTTP_11);
    }

    #[test]
    fn negotiated_version_h2() {
        assert_eq!(negotiated_version(&Connected::new().negotiated_h2()), Version::HTTP_2);
    }

    #[test]
    fn verify_protocol_version_accepts_supported() {
        let base = BaseUri::from_static("https://example.com");
        let supported = vec![Version::HTTP_11, Version::HTTP_2];

        verify_protocol_version(&Connected::new(), &base, &supported).unwrap();
        verify_protocol_version(&Connected::new().negotiated_h2(), &base, &supported).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn verify_protocol_version_rejects_unsupported() {
        let base = BaseUri::from_static("https://example.com");
        let supported = vec![Version::HTTP_2];

        let err = verify_protocol_version(&Connected::new(), &base, &supported).unwrap_err().message();
        insta::assert_snapshot!(err);
    }

    #[test]
    fn verify_protocol_version_skipped_for_single_plaintext_version() {
        // Single supported version + plaintext: ALPN doesn't apply, so the check
        // is intentionally skipped (the server's response framing is what
        // ultimately validates the version).
        let base = BaseUri::from_static("http://example.com");
        let supported = vec![Version::HTTP_2];
        verify_protocol_version(&Connected::new(), &base, &supported).unwrap();
    }

    #[test]
    fn start_event_comes_before_success() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let base = BaseUri::from_static("http://example.com");
        let connector = FakeConnector::new_success(Bytes::new(), clock.clone());

        let (result, events) = connect(&clock, base.clone(), connector.execute(base));

        result.unwrap();
        assert_eq!(
            events,
            [
                ExpectedEvent::new("http.client.connection.start", Severity::Debug)
                    .body("connecting to a remote endpoint")
                    .dimension("server.address", "example.com")
                    .dimension("server.port", 80_u16)
                    .dimension("url.full", "http://example.com/")
                    .dimension("url.scheme", "http"),
                ExpectedEvent::new("http.client.connection.success", Severity::Info)
                    .body("connected to server")
                    .dimension("http.client.connection.setup.duration", 0.0)
                    .dimension("network.protocol.version", "1")
                    .dimension("server.address", "example.com")
                    .dimension("server.port", 80_u16)
                    .dimension("url.full", "http://example.com/")
                    .dimension("url.scheme", "http")
                    .metric(),
            ]
        );
    }

    #[test]
    fn error_event_has_server_fields_label_and_message() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let base = BaseUri::from_static("https://example.com:8443");
        let connector = FakeConnector::new_connect_failure(TestError::new("connection refused"), clock.clone());

        let (result, events) = connect(&clock, base.clone(), connector.execute(base));

        result.unwrap_err();
        assert_eq!(
            events,
            [
                ExpectedEvent::new("http.client.connection.start", Severity::Debug)
                    .body("connecting to a remote endpoint")
                    .dimension("server.address", "example.com")
                    .dimension("server.port", 8443_u16)
                    .dimension("url.full", "https://example.com:8443/")
                    .dimension("url.scheme", "https"),
                ExpectedEvent::new("http.client.connection.error", Severity::Warn)
                    .body("server connection failed")
                    .dimension("error.type", "connect")
                    .dimension("exception.message", "connection refused")
                    .dimension("http.client.connection.setup.duration", 0.0)
                    .dimension("server.address", "example.com")
                    .dimension("server.port", 8443_u16)
                    .dimension("url.full", "https://example.com:8443/")
                    .dimension("url.scheme", "https")
                    .metric(),
            ]
        );
    }

    #[test]
    fn gamma_survivor_connect_failure_and_timeout_both_emit_errors() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let base = BaseUri::from_static("https://example.test");
        let start = || {
            ExpectedEvent::new("http.client.connection.start", Severity::Debug)
                .body("connecting to a remote endpoint")
                .dimension("server.address", "example.test")
                .dimension("server.port", 443_u16)
                .dimension("url.full", "https://example.test/")
                .dimension("url.scheme", "https")
        };

        let (failure, failure_events) = connect(&clock, base.clone(), ready(Err(HttpError::unavailable("failure-marker"))));
        let (timeout, timeout_events) = connect(&clock, base, pending());

        assert!(failure.unwrap_err().to_string().contains("failure-marker"));
        assert!(timeout.unwrap_err().to_string().contains("server connection timeout"));
        assert_eq!(
            failure_events,
            [
                start(),
                ExpectedEvent::new("http.client.connection.error", Severity::Warn)
                    .body("server connection failed")
                    .dimension("error.type", "unavailable")
                    .dimension("exception.message", "failure-marker")
                    .dimension("http.client.connection.setup.duration", 0.0)
                    .dimension("server.address", "example.test")
                    .dimension("server.port", 443_u16)
                    .dimension("url.full", "https://example.test/")
                    .dimension("url.scheme", "https")
                    .metric(),
            ]
        );
        assert_eq!(
            timeout_events,
            [
                start(),
                ExpectedEvent::new("http.client.connection.error", Severity::Warn)
                    .body("server connection failed")
                    .dimension("error.type", "connect")
                    .dimension(
                        "exception.message",
                        "server connection timeout, endpoint: https://example.test/, connection timeout(s): 2"
                    )
                    .dimension("http.client.connection.setup.duration", 2.0)
                    .dimension("server.address", "example.test")
                    .dimension("server.port", 443_u16)
                    .dimension("url.full", "https://example.test/")
                    .dimension("url.scheme", "https")
                    .metric(),
            ]
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connect_with_timeout_returns_error_on_connector_failure() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_connect_failure(TestError::new("boom"), clock.clone());
        let base = BaseUri::from_static("http://example.com");
        let err = connect_with_timeout(
            layered::Service::execute(&connector, base.clone()),
            base,
            &clock,
            Duration::from_secs(5),
            &Sink::noop(),
        )
        .await
        .expect_err("connector failure should propagate");
        assert!(err.to_string().contains("boom"));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connect_with_timeout_returns_timeout_when_connect_too_slow() {
        use seatbelt::{Recovery, RecoveryKind};

        let control = tick::ClockControl::new().auto_advance_timers(true);
        let clock = control.to_clock();
        let base = BaseUri::from_static("http://example.com");
        // pending() never resolves, so the timeout always wins.
        let hanging = pending::<Result<crate::testing::FakeStream>>();
        let err = connect_with_timeout(hanging, base.clone(), &clock, Duration::from_secs(1), &Sink::noop())
            .await
            .expect_err("connect should time out");
        let msg = err.to_string();
        assert!(msg.contains("timeout"), "got: {msg}");
        assert!(msg.contains("connection timeout"), "got: {msg}");
        assert_eq!(err.recovery().kind(), RecoveryKind::Retry);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn client_connector_execute_returns_tracked_stream_on_success() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_success(Bytes::new(), clock.clone());
        let cc: ClientConnector<FakeConnector, crate::testing::FakeStream> = ClientConnector::new(
            connector,
            clock,
            Duration::from_secs(5),
            vec![Version::HTTP_11, Version::HTTP_2],
            Sink::noop(),
            PoolIndex::new(7),
            ConnectionLifetime::fixed(Duration::from_mins(1)),
        );
        let _ = cc.clone(); // exercise Clone impl
        cc.execute(BaseUri::from_static("http://example.com")).await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn client_connector_execute_rejects_unsupported_version() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_success(Bytes::new(), clock.clone());
        // Plaintext + multiple required versions: protocol verification runs.
        let cc: ClientConnector<FakeConnector, crate::testing::FakeStream> = ClientConnector::new(
            connector,
            clock,
            Duration::from_secs(5),
            vec![Version::HTTP_2, Version::HTTP_3],
            Sink::noop(),
            PoolIndex::new(0),
            ConnectionLifetime::unlimited(),
        );
        let err = cc
            .execute(BaseUri::from_static("https://example.com"))
            .await
            .expect_err("HTTP/1.1 should be rejected");
        assert!(err.to_string().contains("unsupported HTTP version"));

        let _ = Bytes::new();
    }

    /// Runs `connect_with_timeout` for `future` with a connect timeout of 2 seconds, and returns its result and the
    /// events that it emitted.
    fn connect(
        clock: &Clock,
        base_uri: BaseUri,
        future: impl Future<Output = Result<FakeStream>>,
    ) -> (Result<FakeStream>, Vec<CapturedEvent>) {
        let (sink, processor) = test_emitter(TEST_ID);
        let result = futures::executor::block_on(connect_with_timeout(future, base_uri, clock, Duration::from_secs(2), &sink));
        (result, processor.events())
    }
}
