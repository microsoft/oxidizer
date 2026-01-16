// Copyright (c) Microsoft Corporation.

use opentelemetry::InstrumentationScope;
use opentelemetry::metrics::{Counter, Meter, MeterProvider};

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

pub(crate) fn create_resilience_event_counter(meter: &Meter) -> Counter<u64> {
    meter
        .u64_counter("resilience.event")
        .with_description("Emitted upon the occurrence of a resilience event.")
        .with_unit("u64")
        .build()
}

#[cfg(test)]
mod tests {
    use opentelemetry_sdk::metrics::InMemoryMetricExporter;

    use super::*;

    #[test]
    #[cfg(not(miri))]
    fn assert_definitions() {
        let exporter = InMemoryMetricExporter::default();
        let meter_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();

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
