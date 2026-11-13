<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Observed Destination Otlp Logo" width="96">

# Observed Destination Otlp

[![crate.io](https://img.shields.io/crates/v/observed_destination_otlp.svg)](https://crates.io/crates/observed_destination_otlp)
[![docs.rs](https://docs.rs/observed_destination_otlp/badge.svg)](https://docs.rs/observed_destination_otlp)
[![MSRV](https://img.shields.io/crates/msrv/observed_destination_otlp)](https://crates.io/crates/observed_destination_otlp)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

OTLP log and metric destinations.

Use these destinations with `observed_otel` or register them
directly on OpenTelemetry SDK provider builders:

* [`otlp_logs`][__link0] sends batched logs.
* [`otlp_metrics`][__link1] sends periodic metrics.

Both builders default to HTTP/protobuf and accept [`OtlpDestinationProtocol`][__link2]
to select gRPC instead.

Metric temporality describes the time range covered by an aggregation:
cumulative covers time since its starting point, while delta covers the interval
since the preceding collection. The export interval controls how often metrics
are sent. See [`Temporality`][__link3] for per-instrument details.

Timeouts, metric intervals, and metric temporality are chosen in this order:

1. An explicit setting on the destination builder.
1. The corresponding OpenTelemetry environment variable.
1. The default: a 10-second export timeout, a 60-second metric interval,
   and cumulative metric temporality.

Environment variables:

* `OTEL_EXPORTER_OTLP_TIMEOUT`: general timeout when no signal-specific timeout is set.
* `OTEL_EXPORTER_OTLP_LOGS_TIMEOUT`: log export timeout; overrides the general timeout.
* `OTEL_EXPORTER_OTLP_METRICS_TIMEOUT`: metric export timeout; overrides the general timeout.
* `OTEL_METRIC_EXPORT_INTERVAL`: interval between metric exports.
* `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE`: metric aggregation temporality.

Timeout and interval environment values are in milliseconds. A zero builder
interval is ignored.

Registering either signal with gRPC requires an entered Tokio runtime with
I/O and timers enabled. Keep that runtime running through provider shutdown.

HTTP export uses Reqwest’s blocking client on the processor’s or reader’s
background thread. Reqwest’s provider-less Rustls backend requires a process-wide crypto
provider before client creation.

Call the SDK provider’s `force_flush` to export pending telemetry immediately,
and call `shutdown` before the application exits. These operations block;
when called from async code, use a blocking task so the runtime can continue
to drive network I/O.

## Example

```rust
use observed_destination_otlp::{OtlpDestinationProtocol, otlp_logs, otlp_metrics};

let logs = otlp_logs("http://localhost:4317")
    .protocol(OtlpDestinationProtocol::Grpc)
    .build();

let metrics = otlp_metrics("http://localhost:4318/v1/metrics").build();
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_destination_otlp">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbt1NN5gl1kZEbVNSqk-d2fKYbtCbhI4vssFQb5nRVEoC5gFVhZIKCeBlvYnNlcnZlZF9kZXN0aW5hdGlvbl9vdGxwZTAuMS4wgnFvcGVudGVsZW1ldHJ5X3Nka2YwLjMzLjE
 [__link0]: https://docs.rs/observed_destination_otlp/0.1.0/observed_destination_otlp/?search=otlp_logs
 [__link1]: https://docs.rs/observed_destination_otlp/0.1.0/observed_destination_otlp/?search=otlp_metrics
 [__link2]: https://docs.rs/observed_destination_otlp/0.1.0/observed_destination_otlp/enum.OtlpDestinationProtocol.html
 [__link3]: https://docs.rs/opentelemetry_sdk/0.33.1/opentelemetry_sdk/?search=metrics::Temporality
