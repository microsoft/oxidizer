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

Add resilience to fallible operations, such as RPC calls over the network, with just a few lines of code.
**Retry** handles transient failures and **Timeout** prevents operations from hanging indefinitely:

```rust
use layered::{Execute, Service, Stack};
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use seatbelt::{RecoveryInfo, ResilienceContext};

let context = ResilienceContext::new(&clock);
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
    .into_service();

let result = service.execute("input data".to_string()).await;
```

## Why?

Communicating over a network is inherently fraught with problems. The network can go down at any time,
sometimes for a millisecond or two. The endpoint you’re connecting to may crash or be rebooted,
network configuration may change from under you, etc. To deliver a robust experience to users, and to
achieve `5` or more `9s` of availability, it is imperative to implement robust resilience patterns to
mask these transient failures.

This crate provides production-ready resilience middleware with excellent telemetry for building
robust distributed systems that can automatically handle timeouts, retries, and other failure
scenarios.

* **Production-ready** - Battle-tested middleware with sensible defaults and comprehensive
  configuration options.
* **Excellent telemetry** - Built-in support for metrics and structured logging to monitor
  resilience behavior in production.
* **Runtime agnostic** - Works seamlessly across any async runtime. Use the same resilience
  patterns across different projects and migrate between runtimes without changes.

## Overview

This crate uses the [`layered`][__link0] crate for composing middleware. The middleware layers
can be stacked together using tuples and built into a service using the [`Stack`][__link1] trait.

Resilience middleware also requires [`Clock`][__link2] from the [`tick`][__link3] crate for timing
operations like delays, timeouts, and backoff calculations. The clock is passed through
[`ResilienceContext`][__link4] when creating middleware layers.

### Core Types

* [`ResilienceContext`][__link5] - Holds shared state for resilience middleware, including the clock.
* [`RecoveryInfo`][__link6] - Classifies errors as recoverable (transient) or non-recoverable (permanent).
* [`Recovery`][__link7] - A trait for types that can determine their recoverability.

### Built-in Middleware

This crate provides built-in resilience middleware that you can use out of the box. See the documentation
for each module for details on how to use them.

* [`timeout`][__link8] - Middleware that cancels long-running operations.
* [`retry`][__link9] - Middleware that automatically retries failed operations.
* [`breaker`][__link10] - Middleware that prevents cascading failures.

## Tower Compatibility

All resilience middleware are compatible with the Tower ecosystem when the `tower-service`
feature is enabled. This allows you to use `tower::ServiceBuilder` to compose middleware stacks:

```rust
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tower::ServiceBuilder;

let context: ResilienceContext<String, Result<String, String>> =
    ResilienceContext::new(&clock);

let service = ServiceBuilder::new()
    .layer(
        Retry::layer("my_retry", &context)
            .clone_input()
            .recovery_with(|result: &Result<String, String>, _| match result {
                Ok(_) => RecoveryInfo::never(),
                Err(_) => RecoveryInfo::retry(),
            }),
    )
    .layer(
        Timeout::layer("my_timeout", &context)
            .timeout(Duration::from_secs(30))
            .timeout_error(|_| "operation timed out".to_string()),
    )
    .service_fn(|input: String| async move {
        Ok::<_, String>(format!("processed: {input}"))
    });
```

## Features

This crate provides several optional features that can be enabled in your `Cargo.toml`:

* **`timeout`** - Enables the [`timeout`][__link11] middleware for canceling long-running operations.
* **`retry`** - Enables the [`retry`][__link12] middleware for automatically retrying failed operations with
  configurable backoff strategies, jitter, and recovery classification.
* **`breaker`** - Enables the [`breaker`][__link13] middleware for preventing cascading failures.
* **`metrics`** - Exposes the OpenTelemetry metrics API for collecting and reporting metrics.
* **`logs`** - Enables structured logging for resilience middleware using the `tracing` crate.
* **`tower-service`** - Enables [`tower_service::Service`][__link14] trait implementations for all
  resilience middleware.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seatbelt">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG4m9a89kCnclG9jGIjV2D_1yGzTaydkW8mVgG84_sI5bv3pNYWSFgmdsYXllcmVkZTAuMy4wgmtyZWNvdmVyYWJsZWUwLjEuMIJoc2VhdGJlbHRlMC4yLjCCZHRpY2tlMC4yLjCCbXRvd2VyX3NlcnZpY2VlMC4zLjM
 [__link0]: https://crates.io/crates/layered/0.3.0
 [__link1]: https://docs.rs/layered/0.3.0/layered/?search=Stack
 [__link10]: https://docs.rs/seatbelt/0.2.0/seatbelt/breaker/index.html
 [__link11]: https://docs.rs/seatbelt/0.2.0/seatbelt/timeout/index.html
 [__link12]: https://docs.rs/seatbelt/0.2.0/seatbelt/retry/index.html
 [__link13]: https://docs.rs/seatbelt/0.2.0/seatbelt/breaker/index.html
 [__link14]: https://docs.rs/tower_service/0.3.3/tower_service/?search=Service
 [__link2]: https://docs.rs/tick/0.2.0/tick/?search=Clock
 [__link3]: https://crates.io/crates/tick/0.2.0
 [__link4]: https://docs.rs/seatbelt/0.2.0/seatbelt/?search=ResilienceContext
 [__link5]: https://docs.rs/seatbelt/0.2.0/seatbelt/?search=ResilienceContext
 [__link6]: https://docs.rs/recoverable/0.1.0/recoverable/?search=RecoveryInfo
 [__link7]: https://docs.rs/recoverable/0.1.0/recoverable/?search=Recovery
 [__link8]: https://docs.rs/seatbelt/0.2.0/seatbelt/timeout/index.html
 [__link9]: https://docs.rs/seatbelt/0.2.0/seatbelt/retry/index.html
