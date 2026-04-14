<div align="center">
 <img src="./logo.png" alt="Error Label Logo" width="96">

# Error Label

[![crate.io](https://img.shields.io/crates/v/error_label.svg)](https://crates.io/crates/error_label)
[![docs.rs](https://docs.rs/error_label/badge.svg)](https://docs.rs/error_label)
[![MSRV](https://img.shields.io/crates/msrv/error_label)](https://crates.io/crates/error_label)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Low-cardinality label for errors, useful for metrics and logging.

This crate provides [`ErrorLabel`][__link0], a low-cardinality string value intended for use as a
metric tag or structured log field. Values should always be chosen from a small, bounded set
known at development time.

## Why

When reporting error telemetry, using the full string representation of an error (e.g. its
[`Display`][__link1] output) as a metric tag or log field leads to high-cardinality
series. Error messages often contain dynamic data such as file paths, URLs, request IDs, or
stack traces, causing the number of distinct tag values to grow without bound. This overwhelms
monitoring systems, inflates storage costs, and makes dashboards unusable.

[`ErrorLabel`][__link2] solves this by giving errors a telemetry-friendly label drawn from a small,
bounded set of values known at development time (e.g. `"timeout"`, `"connection_refused"`).
This keeps metric cardinality predictable while still providing actionable information about
the error.

## Core Types

* [`ErrorLabel`][__link3]: A low-cardinality label for an error, backed by [`Cow<'static, str>`][__link4].

## Examples

```rust
use error_label::ErrorLabel;

// From a static string
let label: ErrorLabel = "timeout".into();
assert_eq!(label, "timeout");

// Dotted chain from parts
let label = ErrorLabel::from_parts(["http", "client", "timeout"]);
assert_eq!(label, "http.client.timeout");
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/error_label">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG8cIzHM6pssmG9mTC7mQLwMJGwt1FTKkMY8xG8Uzz-6Fn7wyYWSBgmtlcnJvcl9sYWJlbGUwLjEuMA
 [__link0]: https://docs.rs/error_label/0.1.0/error_label/struct.ErrorLabel.html
 [__link1]: https://doc.rust-lang.org/stable/std/?search=fmt::Display
 [__link2]: https://docs.rs/error_label/0.1.0/error_label/struct.ErrorLabel.html
 [__link3]: https://docs.rs/error_label/0.1.0/error_label/struct.ErrorLabel.html
 [__link4]: https://doc.rust-lang.org/stable/std/?search=borrow::Cow
