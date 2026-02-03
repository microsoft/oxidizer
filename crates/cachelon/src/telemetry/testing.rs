// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test utilities for telemetry validation.

use std::io::Write;
use std::sync::{Arc, Mutex};

use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, Metric, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use tracing_subscriber::fmt::MakeWriter;

/// Test helper for collecting and asserting on `OTel` metrics.
#[derive(Debug)]
pub(crate) struct MetricTester {
    exporter: InMemoryMetricExporter,
    provider: SdkMeterProvider,
}

impl Default for MetricTester {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricTester {
    #[must_use]
    pub fn new() -> Self {
        let in_memory = InMemoryMetricExporter::default();

        Self {
            exporter: in_memory.clone(),
            provider: SdkMeterProvider::builder().with_periodic_exporter(in_memory).build(),
        }
    }

    #[must_use]
    pub fn meter_provider(&self) -> &SdkMeterProvider {
        &self.provider
    }

    #[must_use]
    pub fn collect_attributes(&self) -> Vec<KeyValue> {
        self.provider.force_flush().unwrap();
        collect_attributes(&self.exporter)
    }

    pub fn assert_attributes_contain(&self, key_values: &[KeyValue]) {
        let attributes = self.collect_attributes();

        for attr in key_values {
            assert!(
                attributes.contains(attr),
                "attribute {attr:?} not found in collected attributes: {attributes:?}"
            );
        }
    }
}

fn collect_attributes(exporter: &InMemoryMetricExporter) -> Vec<KeyValue> {
    exporter
        .get_finished_metrics()
        .unwrap()
        .iter()
        .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
        .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
        .flat_map(collect_attributes_for_metric)
        .collect()
}

fn collect_attributes_for_metric(metric: &Metric) -> impl Iterator<Item = KeyValue> {
    match metric.data() {
        AggregatedMetrics::F64(data) => match data {
            MetricData::Gauge(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
            MetricData::Sum(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
            MetricData::Histogram(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
            MetricData::ExponentialHistogram(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
        },
        AggregatedMetrics::U64(data) => match data {
            MetricData::Gauge(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
            MetricData::Sum(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
            MetricData::Histogram(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
            MetricData::ExponentialHistogram(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
        },
        AggregatedMetrics::I64(data) => match data {
            MetricData::Gauge(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
            MetricData::Sum(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
            MetricData::Histogram(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
            MetricData::ExponentialHistogram(data) => data.data_points().flat_map(|v| v.attributes().cloned()).collect::<Vec<_>>(),
        },
    }
    .into_iter()
}

/// Thread-local log capture buffer for testing.
///
/// Uses `tracing_subscriber::fmt::MakeWriter` to capture formatted log output
/// into a thread-local buffer that can be inspected in tests.
#[derive(Debug, Clone, Default)]
pub(crate) struct LogCapture {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl LogCapture {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Returns the captured log output as a string.
    #[must_use]
    pub fn output(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).to_string()
    }

    /// Asserts that the captured log output contains the given string.
    pub fn assert_contains(&self, expected: &str) {
        let output = self.output();
        assert!(
            output.contains(expected),
            "log output does not contain '{expected}', got:\n{output}"
        );
    }

    /// Creates a `tracing_subscriber` that writes to this capture buffer.
    /// Use with `set_default()` for thread-local capture.
    #[must_use]
    pub fn subscriber(&self) -> impl tracing::Subscriber {
        use tracing_subscriber::layer::SubscriberExt;
        tracing_subscriber::registry().with(tracing_subscriber::fmt::layer().with_writer(self.clone()).with_ansi(false))
    }
}

impl<'a> MakeWriter<'a> for LogCapture {
    type Writer = LogCaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogCaptureWriter {
            buffer: Arc::clone(&self.buffer),
        }
    }
}

/// Writer that appends to a shared buffer.
pub(crate) struct LogCaptureWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl Write for LogCaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
