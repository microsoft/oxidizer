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

Resilience and recovery mechanisms for fallible operations.

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
 > **Note**: Resilience middleware requires [`Clock`][__link0] from the [`tick`][__link1] crate for timing
 > operations like delays, timeouts, and backoff calculations. The clock is passed through
 > [`Context`][__link2] when creating middleware layers.

 > 
 > **Note**: This crate uses the [`layered`][__link3] crate for composing middleware. The middleware layers
 > can be stacked together using tuples and built into a service using the [`Stack`][__link4] trait.

## Why?

This crate provides production-ready resilience middleware with excellent telemetry for building
robust distributed systems that can automatically handle timeouts, retries, and other failure
scenarios.

* **Runtime agnostic** - Works seamlessly across any async runtime. Use the same resilience
  patterns across different projects and migrate between runtimes without changes.
* **Production-ready** - Battle-tested middleware with sensible defaults and comprehensive
  configuration options.
* **Excellent telemetry** - Built-in support for metrics and structured logging to monitor
  resilience behavior in production.

## Overview

### Core Types

* [`Context`][__link5] - Holds shared state for resilience middleware, including the clock.
* [`RecoveryInfo`][__link6] - Classifies errors as recoverable (transient) or non-recoverable (permanent).
* [`Recovery`][__link7] - A trait for types that can determine their recoverability.

### Built-in Middleware

This crate provides built-in resilience middleware that you can use out of the box. See the documentation
for each module for details on how to use them.

* [`timeout`][__link8] - Middleware that cancels long-running operations.
* [`retry`][__link9] - Middleware that automatically retries failed operations.
* [`circuit`][__link10] - Middleware that prevents cascading failures.

## Features

This crate provides several optional features that can be enabled in your `Cargo.toml`:

* **`timeout`** - Enables the [`timeout`][__link11] middleware for canceling long-running operations.
* **`retry`** - Enables the [`retry`][__link12] middleware for automatically retrying failed operations with
  configurable backoff strategies, jitter, and recovery classification.
* **`circuit`** - Enables the [`circuit`][__link13] middleware for preventing cascading failures.
* **`metrics`** - Exposes the OpenTelemetry metrics API for collecting and reporting metrics.
* **`logs`** - Enables structured logging for resilience middleware using the `tracing` crate.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seatbelt">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG8-jxRLAdSUYGz8tFJ1hv_siG0AGq6ysYdCHGyn4hzr6iA3-YWSEgmdsYXllcmVkZTAuMS4wgmtyZWNvdmVyYWJsZWUwLjEuMIJoc2VhdGJlbHRlMC4yLjCCZHRpY2tlMC4xLjI
 [__link0]: https://docs.rs/tick/0.1.2/tick/?search=Clock
 [__link1]: https://crates.io/crates/tick/0.1.2
 [__link10]: https://docs.rs/seatbelt/0.2.0/seatbelt/circuit/index.html
 [__link11]: https://docs.rs/seatbelt/0.2.0/seatbelt/timeout/index.html
 [__link12]: https://docs.rs/seatbelt/0.2.0/seatbelt/retry/index.html
 [__link13]: https://docs.rs/seatbelt/0.2.0/seatbelt/circuit/index.html
 [__link2]: https://docs.rs/seatbelt/0.2.0/seatbelt/?search=shared::Context
 [__link3]: https://crates.io/crates/layered/0.1.0
 [__link4]: https://docs.rs/layered/0.1.0/layered/?search=Stack
 [__link5]: https://docs.rs/seatbelt/0.2.0/seatbelt/?search=shared::Context
 [__link6]: https://docs.rs/recoverable/0.1.0/recoverable/?search=RecoveryInfo
 [__link7]: https://docs.rs/recoverable/0.1.0/recoverable/?search=Recovery
 [__link8]: https://docs.rs/seatbelt/0.2.0/seatbelt/timeout/index.html
 [__link9]: https://docs.rs/seatbelt/0.2.0/seatbelt/retry/index.html
