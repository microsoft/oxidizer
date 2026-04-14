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

[`ErrorLabel`][__link0] wraps a [`Cow<'static, str>`][__link1] to hold either a static string literal
or a heap-allocated [`String`][__link2]. It is intended for use as a metric tag value or
structured log field and should always be chosen from a small, bounded set of values.

## Quick Start

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

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG7MvK6yrcSYDG7k0gIa7vehqG4_QM4ZblxdqG733hAQlMaHtYWSBgmtlcnJvcl9sYWJlbGUwLjEuMA
 [__link0]: https://docs.rs/error_label/0.1.0/error_label/struct.ErrorLabel.html
 [__link1]: https://doc.rust-lang.org/stable/std/?search=borrow::Cow
 [__link2]: https://doc.rust-lang.org/stable/std/string/struct.String.html
