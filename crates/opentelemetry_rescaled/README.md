<div align="center">
 <img src="./logo.png" alt="Opentelemetry Rescaled Logo" width="96">

# Opentelemetry Rescaled

[![crate.io](https://img.shields.io/crates/v/opentelemetry_rescaled.svg)](https://crates.io/crates/opentelemetry_rescaled)
[![docs.rs](https://docs.rs/opentelemetry_rescaled/badge.svg)](https://docs.rs/opentelemetry_rescaled)
[![MSRV](https://img.shields.io/crates/msrv/opentelemetry_rescaled)](https://crates.io/crates/opentelemetry_rescaled)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Wraps an inner OpenTelemetry meter provider to transparently emit *rescaled*
side-by-side copies of selected instruments.

For a chosen instrument in a chosen instrumentation scope, this layer creates
a second instrument whose measurements are the original values multiplied by a
fixed factor. For example, a `http.client.request.duration` instrument that
records seconds can gain a `http.client.request.duration.millis` sidecar that
records the same measurements multiplied by `1000.0`.

The rescaling is invisible to instrument users — they interact only with their
original instrument — and the inner provider simply sees two independently
registered instruments.

## Quick start

```rust
use opentelemetry::metrics::MeterProvider;
use opentelemetry_rescaled::RescaledMetrics;

// Any `MeterProvider` works as the inner provider.
let inner = opentelemetry::metrics::noop::NoopMeterProvider::new();

let outer = RescaledMetrics::builder(inner)
    .scope("my_scope_name", |scope| {
        // source name, target name, target unit (mandatory), factor
        scope.rescale(
            "http.client.request.duration",
            "http.client.request.duration.millis",
            "ms",
            1000.0,
        );
    })
    .build();

// `outer` is itself a `MeterProvider`.
let meter = outer.meter("my_scope_name");
let histogram = meter.f64_histogram("http.client.request.duration").build();
histogram.record(1.5, &[]); // recorded as 1.5 s and, on the sidecar, 1500 ms
```

See [`docs/DESIGN.md`][__link0]
for the architecture and the resolved design decisions.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/opentelemetry_rescaled">source code</a>.
</sub>

 [__link0]: https://github.com/microsoft/oxidizer/blob/main/crates/opentelemetry_rescaled/docs/DESIGN.md
