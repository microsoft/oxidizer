<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Observed Destination Core Logo" width="96">

# Observed Destination Core

[![crate.io](https://img.shields.io/crates/v/observed_destination_core.svg)](https://crates.io/crates/observed_destination_core)
[![docs.rs](https://docs.rs/observed_destination_core/badge.svg)](https://docs.rs/observed_destination_core)
[![MSRV](https://img.shields.io/crates/msrv/observed_destination_core)](https://crates.io/crates/observed_destination_core)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

## Observed Destination Core

This crate provides an integration layer between OpenTelemetry and Observed ecosystems.
The central concept is a “destination” that receives log records or metric data.
Each destination implements [`LogDestination`][__link0] or
[`MetricDestination`][__link1] (or both via separate types).

This crate carries only the destination abstractions and the shared building
blocks that destinations are written against. The destinations themselves live
in companion crates such as `observed_destination_stdout`, so an application
depends on exactly the ones it uses — and can implement its own against the
same abstractions when none of them fits.

[`FlushableMetricsProvider`][__link2] extends OpenTelemetry’s meter-provider contract
with explicit flushing. A processor can retain a full
provider, including a raw-measurement forwarding provider, without depending
on a particular SDK implementation or deriving ownership from cached instruments.
Its flush method uses [`DestinationResult`][__link3] so implementations need not
expose an OpenTelemetry SDK’s error type.

## Examples

These examples store exported data in memory. They use the `testing` feature
of `opentelemetry_sdk` for its in-memory exporters.

### Register a log destination

A destination adds an exporter or processor to the provider builder.
This destination exports each log record as soon as the logger emits it.

```rust
use observed_destination_core::{DestinationResult, LogDestination, LoggerProviderBuilderExt};
use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
use opentelemetry_sdk::logs::{InMemoryLogExporter, LoggerProviderBuilder, SdkLoggerProvider};

#[derive(Debug)]
struct MemoryLogs(InMemoryLogExporter);

impl LogDestination for MemoryLogs {
    fn register(
        self,
        builder: LoggerProviderBuilder,
    ) -> DestinationResult<LoggerProviderBuilder> {
        Ok(builder.with_simple_exporter(self.0))
    }
}

let exporter = InMemoryLogExporter::default();
let provider = SdkLoggerProvider::builder()
    .register(MemoryLogs(exporter.clone()))?
    .build();
let logger = provider.logger("example");
let mut record = logger.create_log_record();
record.set_body("request received".into());
logger.emit(record);

assert_eq!(exporter.get_emitted_logs()?.len(), 1);
```

### Register and flush a metric destination

This destination exports metrics at regular intervals.
The `record_and_flush` function accepts any [`FlushableMetricsProvider`][__link4].
It records a measurement and flushes it without depending on a specific SDK.

```rust
use observed_destination_core::{
    DestinationResult, FlushableMetricsProvider, MeterProviderBuilderExt, MetricDestination,
};
use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, MeterProviderBuilder, SdkMeterProvider,
};

#[derive(Debug)]
struct MemoryMetrics(InMemoryMetricExporter);

impl MetricDestination for MemoryMetrics {
    fn register(
        self,
        builder: MeterProviderBuilder,
    ) -> DestinationResult<MeterProviderBuilder> {
        Ok(builder.with_periodic_exporter(self.0))
    }
}

fn record_and_flush(provider: &impl FlushableMetricsProvider) -> DestinationResult<()> {
    let counter = provider.meter("example").u64_counter("requests").build();
    counter.add(1, &[]);
    provider.force_flush()
}

let exporter = InMemoryMetricExporter::default();
let provider = SdkMeterProvider::builder()
    .register(MemoryMetrics(exporter.clone()))?
    .build();
record_and_flush(&provider)?;

assert!(!exporter.get_finished_metrics()?.is_empty());
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_destination_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbTEcA9cGK8xMbYhsfVUSWFyIbE65TR71UOiAbEkLLat3GMb5hZIGCeBlvYnNlcnZlZF9kZXN0aW5hdGlvbl9jb3JlZjAuMjkuMA
 [__link0]: https://docs.rs/observed_destination_core/0.29.0/observed_destination_core/trait.LogDestination.html
 [__link1]: https://docs.rs/observed_destination_core/0.29.0/observed_destination_core/trait.MetricDestination.html
 [__link2]: https://docs.rs/observed_destination_core/0.29.0/observed_destination_core/?search=FlushableMetricsProvider
 [__link3]: https://docs.rs/observed_destination_core/0.29.0/observed_destination_core/type.DestinationResult.html
 [__link4]: https://docs.rs/observed_destination_core/0.29.0/observed_destination_core/?search=FlushableMetricsProvider
