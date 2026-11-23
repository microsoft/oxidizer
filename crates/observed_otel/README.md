<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Observed Otel Logo" width="96">

# Observed Otel

[![crate.io](https://img.shields.io/crates/v/observed_otel.svg)](https://crates.io/crates/observed_otel)
[![docs.rs](https://docs.rs/observed_otel/badge.svg)](https://docs.rs/observed_otel)
[![MSRV](https://img.shields.io/crates/msrv/observed_otel)](https://crates.io/crates/observed_otel)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

## OpenTelemetry sinks for Observed

This crate builds [`observed::Sink`][__link0] values and processor lists from log and metric destinations.
Destination implementations live in companion crates such as `observed_destination_stdout`.
Each sink gets its own `EventProcessor`s via the builder

### Quick start

Use [`observed_otel::builder`][__link1] to attach destinations and
produce an [`observed::Sink`][__link2]:

```rust
use observed_destination_stdout::{stdout_logs, stdout_metrics};

let sink = observed_otel::builder()
    .log(stdout_logs())
    .metric(stdout_metrics())
    .build()?;
// Call `.id(ID)` to set a stable identity for
// `enrich_for(ID, …)` targeting.

// ... run application, drain producers, then flush with `sink.flush()?`.
// Metric processor Drop also flushes best-effort before releasing ownership.
// Automatic log provider shutdown occurs after its last SDK handle is dropped.
```

To fan an event through multiple emitters in one `emit!`, build each
sink independently and compose them with [`observed::Sink::composite`][__link3].

Each sink owns its own enrichment slot; entries pushed on one sink
are not automatically visible on another.

### Custom meter providers

An embedding host can supply a full OpenTelemetry meter provider implementing
[`observed_destination_core::FlushableMetricsProvider`][__link4] to
[`MetricEventProcessor::new`][__link5]. The same processor handles event
metadata, metric dimensions, enrichment, and redaction regardless of the
provider. This supports providers that forward raw measurements to
another runtime without introducing an SDK aggregation pipeline in Rust.

The processor retains the provider independently of its instrument cache.
Sink flush delegates to the provider; processor Drop also flushes best-effort
before releasing its handle and cached instruments. This exports pending
metrics even if another provider clone is retained, without terminating a
pipeline that other consumers may still use. Final cleanup follows the
provider’s own ownership and Drop semantics.

Drain producers and explicitly flush while exporter/runtime resources are
still available to observe failures. Drop cannot report errors and cannot
cover measurements recorded after its flush. A forwarding provider controls
the sending side; the receiver must flush its own aggregation and exporters.

```rust
use data_privacy::RedactionEngine;
use observed::{emit, event};
use observed_otel::MetricEventProcessor;
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::metrics::SdkMeterProvider;

#[event("requests")]
#[counter]
struct Request;

// A forwarding provider can be supplied through the same contract.
let provider = SdkMeterProvider::builder().build();
let processor = MetricEventProcessor::new(
    provider,
    RedactionEngine::builder().build(),
    Some(InstrumentationScope::builder("hosted-library").build()),
);
let sink = observed_otel::builder().processor(processor).build()?;
emit!(sink, Request);
sink.flush()?; // Flushes the retained provider.
drop(sink); // Flushes best-effort and releases this sink's provider ownership.

```

### Log sampling

Log sampling lets your application choose which log records to send. For
example, you can keep error logs while sending fewer routine progress logs.

Implement [`sampling::LogSampler`][__link6] and attach it per log destination with
[`LogDestinationExt::with_sampler`][__link7]. Your sampler receives enriched and
redacted log records.

Return records in a [`sampling::SamplingBatch`][__link8] to send them to that
destination. Return an empty batch to send nothing. A sampler can also save
records and return them on a later call.

Each log keeps the OpenTelemetry context captured before sampling; later
delivery uses that context rather than the caller’s. See
[`sampling::LogSamplingContext`][__link9] for context access and lifetime details.

[`Sink::flush`][__link10] sends the records that
[`sampling::LogSampler::flush`][__link11] returns. See that method for flush and drop rules.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_otel">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbDBRG4Qtf6CMbaScL1ELBk8cbH30BH819sxMboJ5mA46bQX5hZIOCaG9ic2VydmVkZjAuMzAuMYJ4GW9ic2VydmVkX2Rlc3RpbmF0aW9uX2NvcmVmMC4yOS4ygm1vYnNlcnZlZF9vdGVsZTAuMS4w
 [__link0]: https://docs.rs/observed/0.30.1/observed/?search=Sink
 [__link1]: builder()
 [__link10]: https://docs.rs/observed/0.30.1/observed/?search=Sink::flush
 [__link11]: https://docs.rs/observed_otel/0.1.0/observed_otel/?search=sampling::LogSampler::flush
 [__link2]: https://docs.rs/observed/0.30.1/observed/?search=Sink
 [__link3]: https://docs.rs/observed/0.30.1/observed/?search=Sink::composite
 [__link4]: https://docs.rs/observed_destination_core/0.29.2/observed_destination_core/?search=FlushableMetricsProvider
 [__link5]: https://docs.rs/observed_otel/0.1.0/observed_otel/?search=MetricEventProcessor::new
 [__link6]: https://docs.rs/observed_otel/0.1.0/observed_otel/?search=sampling::LogSampler
 [__link7]: https://docs.rs/observed_otel/0.1.0/observed_otel/?search=LogDestinationExt::with_sampler
 [__link8]: https://docs.rs/observed_otel/0.1.0/observed_otel/?search=sampling::SamplingBatch
 [__link9]: https://docs.rs/observed_otel/0.1.0/observed_otel/?search=sampling::LogSamplingContext
