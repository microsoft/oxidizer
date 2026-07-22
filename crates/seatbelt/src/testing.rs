// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll};

use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, Metric, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

use crate::{Recovery, RecoveryInfo};

/// A tower service whose `poll_ready` always returns an error.
///
/// Useful for testing that resilience middleware correctly propagates
/// inner service readiness failures.
#[derive(Clone, Debug)]
pub(crate) struct FailReadyService;

impl tower_service::Service<String> for FailReadyService {
    type Response = String;
    type Error = String;
    type Future = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Err("inner service unavailable".to_string()))
    }

    fn call(&mut self, _req: String) -> Self::Future {
        unreachable!("call should not be invoked when poll_ready fails")
    }
}

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
    pub(crate) fn new() -> Self {
        let in_memory = InMemoryMetricExporter::default();

        Self {
            exporter: in_memory.clone(),
            provider: SdkMeterProvider::builder().with_periodic_exporter(in_memory).build(),
        }
    }

    #[must_use]
    pub(crate) fn meter_provider(&self) -> &SdkMeterProvider {
        &self.provider
    }

    #[must_use]
    pub(crate) fn collect_attributes(&self) -> Vec<KeyValue> {
        self.provider.force_flush().unwrap();
        collect_attributes(&self.exporter)
    }

    pub(crate) fn assert_attributes(&self, key_values: &[KeyValue], expected_length: Option<usize>) {
        let attributes = self.collect_attributes();

        if let Some(expected_length) = expected_length {
            assert_eq!(
                attributes.len(),
                expected_length,
                "expected {} attributes, got {}",
                expected_length,
                attributes.len()
            );
        }

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

// bleh
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

#[derive(Debug)]
pub(crate) struct RecoverableType(RecoveryInfo);

impl Recovery for RecoverableType {
    fn recovery(&self) -> RecoveryInfo {
        self.0.clone()
    }
}

impl From<RecoveryInfo> for RecoverableType {
    fn from(recovery: RecoveryInfo) -> Self {
        Self(recovery)
    }
}
