// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Records a handful of instruments through a [`RescaledMetrics`] provider and
//! prints the resulting metrics to the terminal, so you can see the rescaled
//! sidecars appear next to their originals — and confirm that an instrument
//! without a configured rule gets no sidecar at all.
//!
//! Run it with:
//!
//! ```text
//! cargo run -p opentelemetry_rescaled --example print_metrics
//! ```

use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_rescaled::RescaledMetrics;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

const SCOPE: &str = "example.http.client";

fn main() {
    // A normal SDK provider with an in-memory exporter so we can read the
    // metrics back and print them.
    let exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(exporter.clone()).build();
    let sdk = SdkMeterProvider::builder().with_reader(reader).build();

    // Wrap it: within `SCOPE`, mirror the seconds-based request duration into a
    // millisecond sidecar (values multiplied by 1000). Everything else is left
    // untouched.
    let metrics = RescaledMetrics::builder(sdk.clone())
        .scope(SCOPE, |scope| {
            scope.rescale("http.client.request.duration", "http.client.request.duration.millis", "ms", 1000.0);
        })
        .build();

    let meter = metrics.meter(SCOPE);

    // Configured instrument: gains a `.millis` sidecar automatically.
    let duration = meter
        .f64_histogram("http.client.request.duration")
        .with_unit("s")
        .with_boundaries(vec![0.1, 0.25, 0.5])
        .build();
    duration.record(0.2, &[]);
    duration.record(0.4, &[]);

    // Unconfigured instrument: no sidecar is created for it.
    let requests = meter.u64_counter("http.client.requests").with_unit("{request}").build();
    requests.add(2, &[]);

    // Flush and print whatever the exporter received.
    sdk.force_flush().expect("force_flush should succeed");
    let resource_metrics = exporter.get_finished_metrics().expect("metrics should be available");

    println!("Collected metrics:\n");
    for resource in &resource_metrics {
        for scope in resource.scope_metrics() {
            for metric in scope.metrics() {
                println!("  {:<40} [{:>4}]  {}", metric.name(), metric.unit(), summarize(metric.data()),);
            }
        }
    }

    println!(
        "\nNote how `http.client.request.duration.millis` appears alongside the\n\
         original (values x1000), while `http.client.requests` has no sidecar."
    );
}

/// Renders a one-line summary of a metric's data points for display.
fn summarize(data: &AggregatedMetrics) -> String {
    match data {
        AggregatedMetrics::F64(MetricData::Histogram(hist)) => hist
            .data_points()
            .map(|dp| format!("histogram sum={} count={}", dp.sum(), dp.count()))
            .collect::<Vec<_>>()
            .join(", "),
        AggregatedMetrics::U64(MetricData::Sum(sum)) => sum
            .data_points()
            .map(|dp| format!("sum={}", dp.value()))
            .collect::<Vec<_>>()
            .join(", "),
        other => format!("{other:?}"),
    }
}
