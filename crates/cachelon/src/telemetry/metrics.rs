// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use opentelemetry::{
    InstrumentationScope,
    metrics::{Counter, Gauge, Histogram, Meter, MeterProvider},
};

const METER_NAME: &str = "cachelon";
const VERSION: &str = "v0.1.0";
const SCHEMA_URL: &str = "https://opentelemetry.io/schemas/1.47.0";
const CACHE_EVENT_COUNT_NAME: &str = "cache.event.count";
const CACHE_OPERATION_DURATION_NAME: &str = "cache.operation.duration_ns";
const CACHE_SIZE_NAME: &str = "cache.size";

pub(crate) fn create_meter(meter_provider: &dyn MeterProvider) -> Meter {
    meter_provider.meter_with_scope(
        InstrumentationScope::builder(METER_NAME)
            .with_version(VERSION)
            .with_schema_url(SCHEMA_URL)
            .build(),
    )
}

pub(crate) fn create_event_counter(meter: &Meter) -> Counter<u64> {
    meter
        .u64_counter(CACHE_EVENT_COUNT_NAME)
        .with_description("Cache events")
        .with_unit("{event}")
        .build()
}

pub(crate) fn create_operation_duration_histogram(meter: &Meter) -> Histogram<f64> {
    meter
        .f64_histogram(CACHE_OPERATION_DURATION_NAME)
        .with_description("Cache operation duration")
        .with_unit("s")
        .build()
}

pub(crate) fn create_cache_size_gauge(meter: &Meter) -> Gauge<u64> {
    meter
        .u64_gauge(CACHE_SIZE_NAME)
        .with_description("Number of entries in the cache")
        .with_unit("{entry}")
        .build()
}
