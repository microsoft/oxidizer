<div align="center">
 <img src="./logo.png" alt="Seatbelt Logo" width="96">

# Seatbelt

[![crate.io](https://img.shields.io/crates/v/seatbelt.svg)](https://crates.io/crates/seatbelt)
[![docs.rs](https://docs.rs/seatbelt/badge.svg)](https://docs.rs/seatbelt)
[![MSRV](https://img.shields.io/crates/msrv/seatbelt)](https://crates.io/crates/seatbelt)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Resilience and fault handling for applications and libraries.

This crate helps applications handle transient faults gracefully through composable
resilience patterns. It provides resilience middleware for building robust distributed systems
that can automatically handle timeouts, retries, and other failure scenarios.

## Runtime Agnostic Design

The seatbelt crate is designed to be **runtime agnostic** and works seamlessly across any
async runtime. This flexibility allows you to use the same resilience patterns across
different projects and migrate between runtimes without changing your resilience patterns.

## Core Types

* [`RecoveryInfo`][__link0]: Classifies errors as recoverable (transient) or non-recoverable (permanent).
* [`Recovery`][__link1]: A trait for types that can determine their recoverability.

## Quick Start

Add resilience to your services with just a few lines of code. **Retry** handles transient failures
and **Timeout** prevents operations from hanging indefinitely:

```rust
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use seatbelt::{RecoveryInfo, Context};

let context = Context::new(&clock);
let service = (
    // Retry middleware: Automatically retries failed operations
    Retry::layer("retry", &context)
        .clone_input()
        .recovery_with(|output: &String, _| match output.as_str() {
            "temporary_error" => RecoveryInfo::retry(),
            "operation timed out" => RecoveryInfo::retry(),
            _ => RecoveryInfo::never(),
        }),
    // Timeout middleware: Cancels operations that take too long
    Timeout::layer("timeout", &context)
        .timeout_output(|_| "operation timed out".to_string())
        .timeout(Duration::from_secs(30)),
    // Your core business logic
    Execute::new(my_string_operation),
)
    .build();

let result = service.execute("input data".to_string()).await;
```

 > 
 > **Note**: Resilience middleware requires [`Clock`][__link2] from the [`tick`][__link3] crate for timing
 > operations like delays, timeouts, and backoff calculations. The clock is passed through
 > [`Context`][__link4] when creating middleware layers.

See [Built-in Middlewares](#built-in-middleware) for more details.

## Recovery Metadata

Error types can implement [`Recovery`][__link5] to provide additional metadata about their retry characteristics.
This enables callers to use a unified, streamlined approach when determining whether to retry an
operation, regardless of the underlying error type or source.

## Built-in Middleware

This crate provides built-in resilience middleware that you can use out of the box. See the documentation
for each module for details on how to use them.

* [`timeout`][__link6]: Cancels long-running operations.
* [`retry`][__link7]: Automatically retries failed operations with configurable backoff strategies.
* [`circuit`][__link8]: Prevents cascading failures by stopping requests to unhealthy services.

### Features

This crate supports several optional features that can be enabled to extend functionality:

* `options`: Enables common APIs for building resilience middleware, including [`Context`][__link9].
  Requires [`tick`][__link10] for timing operations.
* `service`: Re-exports common types for building middleware from [`layered`][__link11] crate.
* `telemetry`: Enables telemetry and observability features using OpenTelemetry for monitoring
  resilience operations.
* `metrics`: Exposes the OpenTelemetry metrics API for collecting and reporting metrics.
* `timeout`: Enables the [`timeout`][__link12] middleware for canceling long-running operations.
* `retry`: Enables the [`retry`][__link13] middleware for automatically retrying failed operations with
  configurable backoff strategies, jitter, and recovery classification.
* `circuit`: Enables the [`circuit`][__link14] middleware for preventing cascading failures.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seatbelt">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG2dhk5l0Ky--G-zcUMSmDnfBG6s2B3IYE_rxGzkEJPiyOJeqYWSEgmdsYXllcmVkZTAuMS4wgmtyZWNvdmVyYWJsZWUwLjEuMIJoc2VhdGJlbHRlMC4xLjCCZHRpY2tlMC4xLjI
 [__link0]: https://docs.rs/recoverable/0.1.0/recoverable/?search=RecoveryInfo
 [__link1]: https://docs.rs/recoverable/0.1.0/recoverable/?search=Recovery
 [__link10]: https://crates.io/crates/tick/0.1.2
 [__link11]: https://crates.io/crates/layered/0.1.0
 [__link12]: https://docs.rs/seatbelt/0.1.0/seatbelt/timeout/index.html
 [__link13]: https://docs.rs/seatbelt/0.1.0/seatbelt/retry/index.html
 [__link14]: https://docs.rs/seatbelt/0.1.0/seatbelt/circuit/index.html
 [__link2]: https://docs.rs/tick/0.1.2/tick/?search=Clock
 [__link3]: https://crates.io/crates/tick/0.1.2
 [__link4]: https://docs.rs/seatbelt/0.1.0/seatbelt/?search=options::Context
 [__link5]: https://docs.rs/recoverable/0.1.0/recoverable/?search=Recovery
 [__link6]: https://docs.rs/seatbelt/0.1.0/seatbelt/timeout/index.html
 [__link7]: https://docs.rs/seatbelt/0.1.0/seatbelt/retry/index.html
 [__link8]: https://docs.rs/seatbelt/0.1.0/seatbelt/circuit/index.html
 [__link9]: https://docs.rs/seatbelt/0.1.0/seatbelt/?search=options::Context
