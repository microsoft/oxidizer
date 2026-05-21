// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`ClientConnector`]: layered service producing a [`TrackedStream`] for
//! every successful connect, applying connect-timeout and HTTP-version
//! verification along the way.

use std::fmt::Display;
use std::marker::PhantomData;
use std::time::Duration;

use http::Version;
use http_extensions::{HttpError, Result};
use hyper_util::client::legacy::connect::{Connected, Connection};
use ohno::ErrorLabel;
use opentelemetry::metrics::{Histogram, Meter};
use seatbelt::RecoveryInfo;
use templated_uri::BaseUri;
use tick::{Clock, FutureExt};
use tracing::{Level, event};

use crate::connection::connect::Connect;
use crate::connection::io::HyperIo;
use crate::connection::tracked_stream::TrackedStream;
use crate::error_labels::{LABEL_CONNECT, LABEL_HTTP_VERSION_UNSUPPORTED, collect_error_labels};
use crate::options::ConnectionLifetime;
use crate::telemetry::{ConnectionInfo, create_connection_attributes, create_connection_failure_attributes};

/// A connector service that applies connect-timeout, version verification,
/// and lifecycle tracking on top of a user-supplied [`Connect`].
pub(crate) struct ClientConnector<C, S> {
    connector: C,
    clock: Clock,
    connect_timeout: Duration,
    supported_versions: Vec<Version>,
    connection_setup_duration: Histogram<f64>,
    connection_duration: Histogram<f64>,
    pool_index: usize,
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
            connection_setup_duration: self.connection_setup_duration.clone(),
            connection_duration: self.connection_duration.clone(),
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
        meter: &Meter,
        pool_index: usize,
        connection_lifetime: ConnectionLifetime,
    ) -> Self {
        Self {
            connector,
            clock,
            connect_timeout,
            supported_versions,
            connection_setup_duration: meter
                .f64_histogram("http.client.connection.setup.duration")
                .with_description("The duration of setting up the connection.")
                .with_unit("s")
                .with_boundaries(vec![
                    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0, 25.0, 50.0,
                ])
                .build(),
            connection_duration: meter
                .f64_histogram("http.client.connection.duration")
                .with_description("The total duration of the connection from establishment to close.")
                .with_unit("s")
                .with_boundaries(vec![0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0])
                .build(),
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
            self.connection_setup_duration.clone(),
        )
        .await?;

        let connected = connection.connected();
        verify_protocol_version(&connected, &input, &self.supported_versions)?;

        Ok(TrackedStream::new(
            connection,
            input,
            ConnectionInfo::new(&self.clock, self.pool_index, max_age),
            self.connection_duration.clone(),
            connected,
        ))
    }
}

async fn connect_with_timeout<S>(
    future: impl Future<Output = Result<S>>,
    base_uri: BaseUri,
    clock: &Clock,
    connect_timeout: Duration,
    connection_setup_duration: Histogram<f64>,
) -> Result<S>
where
    S: Connection,
{
    event!(
        name: "http.client.connection.start",
        Level::DEBUG,
        server.address = base_uri.authority().as_str(),
        server.port = base_uri.port(),
        url.scheme = %base_uri.scheme(),
        url.full = %base_uri,
        "connecting to a remote endpoint",
    );

    let stopwatch = clock.stopwatch();
    let result = future.timeout(clock, connect_timeout).await;
    let elapsed = stopwatch.elapsed();

    match result {
        Ok(Ok(connection)) => {
            connection_setup_duration.record(
                elapsed.as_secs_f64(),
                &create_connection_attributes(&base_uri, &connection.connected()),
            );

            event!(
                name: "http.client.connection.success",
                Level::INFO,
                server.address = base_uri.authority().as_str(),
                server.port = base_uri.port(),
                url.scheme = %base_uri.scheme(),
                url.full = %base_uri,
                http.connection.setup.duration = elapsed.as_secs_f64(),
                "connected to server",
            );

            Ok(connection)
        }
        Ok(Err(error)) => {
            report_connection_error(&base_uri, elapsed, &connection_setup_duration, collect_error_labels(&error), &error);

            Err(error)
        }
        Err(_timeout_error) => {
            let message = format!(
                "server connection timeout, endpoint: {base_uri}, connection timeout(s): {}",
                connect_timeout.as_secs(),
            );

            report_connection_error(&base_uri, elapsed, &connection_setup_duration, LABEL_CONNECT, &message);

            Err(HttpError::other(message, RecoveryInfo::retry(), LABEL_CONNECT))
        }
    }
}

fn report_connection_error(
    base_uri: &BaseUri,
    elapsed: Duration,
    connection_setup_duration: &Histogram<f64>,
    error_label: ErrorLabel,
    error: &impl Display,
) {
    event!(
        name: "http.client.connection.error",
        Level::WARN,
        server.address = base_uri.authority().as_str(),
        server.port = base_uri.port(),
        url.scheme = %base_uri.scheme(),
        url.full = %base_uri,
        http.connection.setup.duration = elapsed.as_secs_f64(),
        error.type = %error_label,
        error = %error,
        "server connection failed",
    );

    connection_setup_duration.record(elapsed.as_secs_f64(), &create_connection_failure_attributes(base_uri, error_label));
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
    use ohno::ErrorExt;
    use opentelemetry::KeyValue;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use testing_aids::MetricTester;

    use super::*;

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
    #[cfg_attr(miri, ignore)]
    #[tracing_test::traced_test]
    fn report_connection_error_records_metric() {
        let tester = MetricTester::new();
        let histogram = tester
            .meter_provider()
            .meter("test")
            .f64_histogram("http.client.connection.setup.duration")
            .build();

        report_connection_error(
            &BaseUri::from_static("https://example.com:8443"),
            Duration::from_millis(150),
            &histogram,
            LABEL_CONNECT,
            &"connection refused",
        );

        tester.assert_attributes_contain(&[
            KeyValue::new("server.address", "example.com"),
            KeyValue::new("server.port", 8443_i64),
            KeyValue::new("error.type", "connect"),
        ]);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn connect_with_timeout_records_metric_on_success() {
        use bytes::Bytes;

        use crate::testing::FakeConnector;

        let tester = MetricTester::new();
        let histogram = tester
            .meter_provider()
            .meter("test")
            .f64_histogram("http.client.connection.setup.duration")
            .build();

        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_success(Bytes::new(), clock.clone());
        let base = BaseUri::from_static("http://example.com");
        let result = connect_with_timeout(
            layered::Service::execute(&connector, base.clone()),
            base,
            &clock,
            Duration::from_secs(5),
            histogram.clone(),
        )
        .await;
        result.unwrap();

        tester.assert_attributes_contain(&[KeyValue::new("server.address", "example.com")]);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connect_with_timeout_returns_error_on_connector_failure() {
        use crate::testing::{FakeConnector, TestError};

        let provider = SdkMeterProvider::builder().build();
        let histogram = provider
            .meter("test")
            .f64_histogram("http.client.connection.setup.duration")
            .build();

        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_connect_failure(TestError::new("boom"), clock.clone());
        let base = BaseUri::from_static("http://example.com");
        let err = connect_with_timeout(
            layered::Service::execute(&connector, base.clone()),
            base,
            &clock,
            Duration::from_secs(5),
            histogram,
        )
        .await
        .expect_err("connector failure should propagate");
        assert!(err.to_string().contains("boom"));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connect_with_timeout_returns_timeout_when_connect_too_slow() {
        use std::future::pending;

        use seatbelt::{Recovery, RecoveryKind};

        let provider = SdkMeterProvider::builder().build();
        let histogram = provider
            .meter("test")
            .f64_histogram("http.client.connection.setup.duration")
            .build();

        let control = tick::ClockControl::new().auto_advance_timers(true);
        let clock = control.to_clock();
        let base = BaseUri::from_static("http://example.com");
        // pending() never resolves, so the timeout always wins.
        let hanging = pending::<Result<crate::testing::FakeStream>>();
        let err = connect_with_timeout(hanging, base.clone(), &clock, Duration::from_secs(1), histogram)
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
        use bytes::Bytes;
        use layered::Service as _;

        use crate::testing::FakeConnector;

        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_success(Bytes::new(), clock.clone());
        let provider = SdkMeterProvider::builder().build();
        let meter = provider.meter("test");
        let cc: ClientConnector<FakeConnector, crate::testing::FakeStream> = ClientConnector::new(
            connector,
            clock,
            Duration::from_secs(5),
            vec![Version::HTTP_11, Version::HTTP_2],
            &meter,
            7,
            ConnectionLifetime::Fixed(Duration::from_secs(60)),
        );
        let _ = cc.clone(); // exercise Clone impl
        cc.execute(BaseUri::from_static("http://example.com")).await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn client_connector_execute_rejects_unsupported_version() {
        use bytes::Bytes;
        use layered::Service as _;

        use crate::testing::FakeConnector;

        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_success(Bytes::new(), clock.clone());
        let provider = SdkMeterProvider::builder().build();
        let meter = provider.meter("test");
        // Plaintext + multiple required versions: protocol verification runs.
        let cc: ClientConnector<FakeConnector, crate::testing::FakeStream> = ClientConnector::new(
            connector,
            clock,
            Duration::from_secs(5),
            vec![Version::HTTP_2, Version::HTTP_3],
            &meter,
            0,
            ConnectionLifetime::Unlimited,
        );
        let err = cc
            .execute(BaseUri::from_static("https://example.com"))
            .await
            .expect_err("HTTP/1.1 should be rejected");
        assert!(err.to_string().contains("unsupported HTTP version"));

        let _ = Bytes::new();
    }
}
