// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`ClientConnector`]: layered service producing a [`TrackedStream`] for
//! every successful connect, applying connect-timeout and HTTP-version
//! verification along the way.

use std::fmt::Display;
use std::marker::PhantomData;
use std::time::Duration;

use fetch_options::{ConnectionInfo, ConnectionLifetime, PoolIndex};
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
use crate::telemetry::{create_connection_attributes, create_connection_failure_attributes};

/// A connector service that applies connect-timeout, version verification,
/// and lifecycle tracking on top of a user-supplied [`Connect`].
pub(crate) struct ClientConnector<C, S> {
    connector: C,
    clock: Clock,
    connect_timeout: Duration,
    supported_versions: Vec<Version>,
    connection_setup_duration: Histogram<f64>,
    connection_duration: Histogram<f64>,
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
        pool_index: PoolIndex,
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
            ConnectionInfo::new(connection_now(&self.clock), self.pool_index, max_age),
            self.connection_duration.clone(),
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
    connection_setup_duration: Histogram<f64>,
) -> Result<S>
where
    S: Connection,
{
    event!(
        name: "http.client.connection.start",
        Level::DEBUG,
        server.address = base_uri.host(),
        server.port = base_uri.effective_port(),
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
                server.address = base_uri.host(),
                server.port = base_uri.effective_port(),
                url.scheme = %base_uri.scheme(),
                url.full = %base_uri,
                http.connection.setup.duration = elapsed.as_secs_f64(),
                "connected to server",
            );

            Ok(connection)
        }
        Ok(Err(error)) => {
            // #[gamma::skip(call_result.default, tag = "equivalent", reason = "report_connection_error returns unit; default_after preserves metrics and events and returns the same ()")]
            report_connection_error(&base_uri, elapsed, &connection_setup_duration, collect_error_labels(&error), &error);

            Err(error)
        }
        Err(_timeout_error) => {
            let message = format!(
                "server connection timeout, endpoint: {base_uri}, connection timeout(s): {}",
                connect_timeout.as_secs(),
            );

            // #[gamma::skip(call_result.default, tag = "equivalent", reason = "report_connection_error returns unit; default_after preserves metrics and events and returns the same ()")]
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
        server.address = base_uri.host(),
        server.port = base_uri.effective_port(),
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
    use opentelemetry_sdk::metrics::data::{HistogramDataPoint, ResourceMetrics, ScopeMetrics};
    #[test]
    fn gamma_survivor_connect_failure_and_timeout_both_record_metrics() {
        use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
        let histogram = provider.meter("test").f64_histogram("failures").build();
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let base = templated_uri::BaseUri::from_static("https://example.test");
        let failure = futures::executor::block_on(super::connect_with_timeout(
            std::future::ready(Err::<crate::testing::FakeStream, _>(http_extensions::HttpError::unavailable(
                "failure-marker",
            ))),
            base.clone(),
            &clock,
            std::time::Duration::from_secs(2),
            histogram.clone(),
        ))
        .unwrap_err();
        assert!(failure.to_string().contains("failure-marker"));
        let timeout = futures::executor::block_on(super::connect_with_timeout(
            std::future::pending::<http_extensions::Result<crate::testing::FakeStream>>(),
            base,
            &clock,
            std::time::Duration::from_secs(2),
            histogram,
        ))
        .unwrap_err();
        assert!(timeout.to_string().contains("server connection timeout"));
        provider.force_flush().unwrap();
        let resources = exporter.get_finished_metrics().unwrap();
        let metric = resources
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .find(|metric| metric.name() == "failures")
            .unwrap();
        let opentelemetry_sdk::metrics::data::AggregatedMetrics::F64(opentelemetry_sdk::metrics::data::MetricData::Histogram(histogram)) =
            metric.data()
        else {
            panic!("failure samples must be histogram data");
        };
        assert_eq!(histogram.data_points().map(HistogramDataPoint::count).sum::<u64>(), 2);
        assert_eq!(
            histogram.data_points().map(HistogramDataPoint::sum).sum::<f64>().to_bits(),
            2.0_f64.to_bits()
        );
    }
    #[test]
    fn gamma_survivor_connection_metrics_metadata_buckets_and_failure_sample() {
        use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
        let clock = tick::Clock::new_frozen();
        let connector = crate::testing::FakeConnector::new_success(bytes::Bytes::new(), clock.clone());
        let cc: super::ClientConnector<_, crate::testing::FakeStream> = super::ClientConnector::new(
            connector,
            clock,
            std::time::Duration::from_secs(1),
            vec![http::Version::HTTP_11],
            &provider.meter("test"),
            fetch_options::PoolIndex::new(0),
            fetch_options::ConnectionLifetime::unlimited(),
        );
        super::report_connection_error(
            &templated_uri::BaseUri::from_static("https://example.test"),
            std::time::Duration::from_millis(125),
            &cc.connection_setup_duration,
            crate::error_labels::LABEL_CONNECT,
            &"marker",
        );
        cc.connection_duration.record(0.25, &[]);
        provider.force_flush().unwrap();
        let metrics = exporter.get_finished_metrics().unwrap();
        let metrics = metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .collect::<Vec<_>>();
        for (name, description, bounds, sum) in [
            (
                "http.client.connection.setup.duration",
                "The duration of setting up the connection.",
                vec![
                    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0, 25.0, 50.0,
                ],
                0.125_f64,
            ),
            (
                "http.client.connection.duration",
                "The total duration of the connection from establishment to close.",
                vec![0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0],
                0.25,
            ),
        ] {
            let metric = metrics.iter().find(|metric| metric.name() == name).unwrap();
            assert_eq!(metric.description(), description);
            assert_eq!(metric.unit(), "s");
            let opentelemetry_sdk::metrics::data::AggregatedMetrics::F64(opentelemetry_sdk::metrics::data::MetricData::Histogram(
                histogram,
            )) = metric.data()
            else {
                panic!("connection metrics must use an explicit histogram");
            };
            let point = histogram.data_points().next().unwrap();
            assert_eq!(point.bounds().collect::<Vec<_>>(), bounds);
            assert_eq!(point.sum().to_bits(), sum.to_bits());
            assert_eq!(point.count(), 1);
        }
    }
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
            PoolIndex::new(7),
            ConnectionLifetime::fixed(Duration::from_mins(1)),
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
}
