<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Observed Utils Logo" width="96">

# Observed Utils

[![crate.io](https://img.shields.io/crates/v/observed_utils.svg)](https://crates.io/crates/observed_utils)
[![docs.rs](https://docs.rs/observed_utils/badge.svg)](https://docs.rs/observed_utils)
[![MSRV](https://img.shields.io/crates/msrv/observed_utils)](https://crates.io/crates/observed_utils)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Helper types and utilities that expand `observed` functionality.

[`observed`][__link0] itself does not depend on `opentelemetry`: an event is a typed
struct, and a crate that only emits events should not compile an exporter’s
dependency tree. A crate that *does* export to OpenTelemetry needs the
conversion anyway, so it lives here - once, rather than re-derived in every
exporter.

* [`any_value_of`][__link1], [`otel_value_of`][__link2] and [`otel_severity_of`][__link3] convert an
  [`observed::Value`][__link4] or [`observed::Severity`][__link5] into its OpenTelemetry
  counterpart. OpenTelemetry has no unsigned value, so a `u64` converts to an
  `i64` saturating at `i64::MAX` rather than wrapping.
* [`metric_number_of`][__link6] extracts the number a metric instrument records.
* [`format_any_value`][__link7] renders an [`AnyValue`][__link8]
  in human-readable form instead of its `Debug` shape.
* [`SensitiveSlice`][__link9] holds a bounded, type-erased collection of classified
  items inline, without allocating for the collection, and renders them
  through the caller’s redactor.

This crate is less stable than `observed` itself and may have breaking changes.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_utils">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbRrqGsb3cxdEbOysBQ_WLZMwbnd9RXJQ5msMbnMdmNajwvJBhZIOCaG9ic2VydmVkZjAuMjQuMIJub2JzZXJ2ZWRfdXRpbHNlMC4xLjCCbW9wZW50ZWxlbWV0cnlmMC4zMi4w
 [__link0]: https://crates.io/crates/observed/0.24.0
 [__link1]: https://docs.rs/observed_utils/0.1.0/observed_utils/?search=any_value_of
 [__link2]: https://docs.rs/observed_utils/0.1.0/observed_utils/?search=otel_value_of
 [__link3]: https://docs.rs/observed_utils/0.1.0/observed_utils/?search=otel_severity_of
 [__link4]: https://docs.rs/observed/0.24.0/observed/?search=Value
 [__link5]: https://docs.rs/observed/0.24.0/observed/?search=Severity
 [__link6]: https://docs.rs/observed_utils/0.1.0/observed_utils/?search=metric_number_of
 [__link7]: https://docs.rs/observed_utils/0.1.0/observed_utils/?search=format_any_value
 [__link8]: https://docs.rs/opentelemetry/0.32.0/opentelemetry/?search=logs::AnyValue
 [__link9]: https://docs.rs/observed_utils/0.1.0/observed_utils/?search=SensitiveSlice
