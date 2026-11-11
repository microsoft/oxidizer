// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the in-memory metric destination.

use observed_destination_core::MeterProviderBuilderExt;
use observed_destination_memory::memory_metrics;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

#[test]
#[cfg_attr(
    miri,
    ignore = "opentelemetry_sdk metric aggregation reads the realtime clock, unavailable under Miri isolation; native and cargo careful runs retain this coverage"
)]
fn metric_destination_exports_recorded_values_on_flush() {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .register(memory_metrics(exporter.clone()))
        .unwrap()
        .build();
    let counter = provider.meter("memory").u64_counter("requests").build();

    counter.add(2, &[]);
    provider.force_flush().unwrap();

    let batches = exporter.get_finished_metrics().unwrap();
    let metric = batches
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(ScopeMetrics::metrics)
        .find(|metric| metric.name() == "requests")
        .unwrap();
    let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
        panic!("expected a u64 counter");
    };
    assert_eq!(sum.data_points().next().unwrap().value(), 2);

    provider.shutdown().unwrap();
}
