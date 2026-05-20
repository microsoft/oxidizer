// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`TrackedStream`]: wraps a stream to record an
//! `http.client.connection.duration` histogram on drop and to attach the
//! [`ConnectionInfo`] response extension.

use std::io::IoSlice;
use std::pin::Pin;
use std::task::{Context, Poll};

use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use opentelemetry::metrics::Histogram;
use templated_uri::BaseUri;
use tracing::{Level, event};

use crate::telemetry::{ConnectionInfo, create_connection_attributes};

/// A wrapper around a stream that tracks connection lifecycle.
#[derive(Debug)]
pub(crate) struct TrackedStream<T> {
    inner: T,
    base_uri: BaseUri,
    info: ConnectionInfo,
    connection_duration: Option<Histogram<f64>>,
    connected: Connected,
}

impl<T> TrackedStream<T> {
    pub(crate) fn new(
        inner: T,
        base_uri: BaseUri,
        info: ConnectionInfo,
        connection_duration: Histogram<f64>,
        connected: Connected,
    ) -> Self {
        Self {
            inner,
            base_uri,
            info,
            connection_duration: Some(connection_duration),
            connected,
        }
    }
}

impl<T> Drop for TrackedStream<T> {
    fn drop(&mut self) {
        let Some(connection_duration) = self.connection_duration.take() else {
            return;
        };

        let duration_secs = self.info.age().as_secs_f64();

        connection_duration.record(duration_secs, &create_connection_attributes(&self.base_uri, &self.connected));

        event!(
            name: "http.client.connection.closed",
            Level::DEBUG,
            server.address = self.base_uri.host(),
            server.port = self.base_uri.port(),
            url.scheme = %self.base_uri.scheme(),
            http.connection.duration = duration_secs,
            "connection closed",
        );
    }
}

impl<T: Read + Unpin> Read for TrackedStream<T> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: Write + Unpin> Write for TrackedStream<T> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[IoSlice<'_>]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
}

impl<T: Connection> Connection for TrackedStream<T> {
    fn connected(&self) -> Connected {
        self.inner.connected().extra(self.info.clone())
    }
}

impl<T: Unpin> Unpin for TrackedStream<T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::pin;
    use std::task::Waker;

    use opentelemetry::KeyValue;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
    use tick::Clock;

    use super::*;
    use crate::testing::PanickingStream;

    fn make_histogram() -> Histogram<f64> {
        SdkMeterProvider::builder()
            .build()
            .meter("test")
            .f64_histogram("http.client.connection.duration")
            .build()
    }

    fn create() -> TrackedStream<PanickingStream> {
        TrackedStream {
            inner: PanickingStream,
            base_uri: BaseUri::from_static("https://example.com"),
            info: ConnectionInfo::new(&Clock::new_frozen(), 0, None),
            connection_duration: None,
            connected: Connected::new(),
        }
    }

    #[test]
    fn drop_without_histogram_is_noop() {
        drop(create());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn drop_records_metric_with_inferred_port() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();
        let histogram = provider.meter("test").f64_histogram("http.client.connection.duration").build();

        drop(TrackedStream {
            inner: PanickingStream,
            base_uri: BaseUri::from_static("https://example.com"),
            info: ConnectionInfo::new(&Clock::new_frozen(), 0, None),
            connection_duration: Some(histogram),
            connected: Connected::new(),
        });
        provider.force_flush().unwrap();

        let metrics = exporter.get_finished_metrics().unwrap();
        let metric = metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(ScopeMetrics::metrics)
            .find(|m| m.name() == "http.client.connection.duration")
            .expect("metric not found");

        let attrs: Vec<KeyValue> = match metric.data() {
            AggregatedMetrics::F64(MetricData::Histogram(h)) => h.data_points().flat_map(|dp| dp.attributes().cloned()).collect(),
            _ => panic!("unexpected metric type"),
        };

        assert!(attrs.contains(&KeyValue::new("server.address", "example.com")));
        assert!(attrs.contains(&KeyValue::new("server.port", 443_i64)));
    }

    #[test]
    fn connected_does_not_panic_for_well_behaved_inner() {
        let stream = TrackedStream {
            inner: ConnectedOnlyStream,
            base_uri: BaseUri::from_static("https://example.com"),
            info: ConnectionInfo::new(&Clock::new_frozen(), 7, None),
            connection_duration: None,
            connected: Connected::new(),
        };

        let _connected = stream.connected();
    }

    /// Minimal stream type whose `connected()` does NOT panic, allowing us
    /// to assert that `TrackedStream` extends the inner metadata rather than
    /// replacing it.
    struct ConnectedOnlyStream;

    impl Read for ConnectedOnlyStream {
        fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
            unreachable!()
        }
    }

    impl Write for ConnectedOnlyStream {
        fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: &[u8]) -> Poll<std::io::Result<usize>> {
            unreachable!()
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            unreachable!()
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            unreachable!()
        }
    }

    impl Connection for ConnectedOnlyStream {
        fn connected(&self) -> Connected {
            Connected::new()
        }
    }

    #[should_panic(expected = "poll_read")]
    #[test]
    fn poll_read_delegates_to_inner() {
        let mut cx = Context::from_waker(Waker::noop());
        let stream = pin!(create());
        let mut buf = hyper::rt::ReadBuf::uninit(&mut []);
        let _ = stream.poll_read(&mut cx, buf.unfilled());
    }

    #[should_panic(expected = "poll_write")]
    #[test]
    fn poll_write_delegates_to_inner() {
        let mut cx = Context::from_waker(Waker::noop());
        let stream = pin!(create());
        let _ = stream.poll_write(&mut cx, b"x");
    }

    #[should_panic(expected = "poll_flush")]
    #[test]
    fn poll_flush_delegates_to_inner() {
        let mut cx = Context::from_waker(Waker::noop());
        let stream = pin!(create());
        let _ = stream.poll_flush(&mut cx);
    }

    #[should_panic(expected = "poll_shutdown")]
    #[test]
    fn poll_shutdown_delegates_to_inner() {
        let mut cx = Context::from_waker(Waker::noop());
        let stream = pin!(create());
        let _ = stream.poll_shutdown(&mut cx);
    }

    #[should_panic(expected = "is_write_vectored")]
    #[test]
    fn is_write_vectored_delegates_to_inner() {
        let stream = create();
        let _ = stream.is_write_vectored();
    }

    #[test]
    fn new_starts_with_active_histogram() {
        let stream = TrackedStream::new(
            PanickingStream,
            BaseUri::from_static("https://example.com"),
            ConnectionInfo::new(&Clock::new_frozen(), 0, None),
            make_histogram(),
            Connected::new(),
        );
        assert!(stream.connection_duration.is_some());
    }
}
