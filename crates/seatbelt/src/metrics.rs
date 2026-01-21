// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use opentelemetry::InstrumentationScope;
use opentelemetry::metrics::{Meter, MeterProvider};

const METER_NAME: &str = "seatbelt";
const VERSION: &str = "v0.1.0";
const SCHEMA_URL: &str = "https://opentelemetry.io/schemas/1.47.0";

pub(crate) fn create_meter(meter_provider: &dyn MeterProvider) -> Meter {
    meter_provider.meter_with_scope(
        InstrumentationScope::builder(METER_NAME)
            .with_version(VERSION)
            .with_schema_url(SCHEMA_URL)
            .build(),
    )
}

#[cfg(any(feature = "retry", feature = "circuit-breaker", feature = "timeout", test))]
pub(crate) fn create_resilience_event_counter(meter: &Meter) -> opentelemetry::metrics::Counter<u64> {
    meter
        .u64_counter("resilience.event")
        .with_description("Emitted upon the occurrence of a resilience event.")
        .with_unit("u64")
        .build()
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
#[cfg(not(miri))]
mod tests {
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    use super::*;

    #[test]
    fn assert_definitions() {
        let exporter = InMemoryMetricExporter::default();
        let meter_provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();

        let meter = create_meter(&meter_provider);
        let resilience_events = create_resilience_event_counter(&meter);
        resilience_events.add(1, &[]);

        meter_provider.force_flush().unwrap();

        let metrics = exporter.get_finished_metrics().unwrap();
        let str = format!("{metrics:?}");

        assert!(str.contains("resilience.event"));
        assert!(str.contains("u64"));
        assert!(str.contains("seatbelt"));
        assert!(str.contains("v0.1.0"));
        assert!(str.contains("https://opentelemetry.io/schemas/1.47"));
    }
}
