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
* [`hedging`][__link10] - Middleware that reduces tail latency via additional concurrent execution.
* [`breaker`][__link11] - Middleware that prevents cascading failures.
* [`fallback`][__link12] - Middleware that replaces invalid output with a user-defined alternative.

## Middleware Ordering

The order in which resilience middleware is composed **matters**. Layers apply outer to inner
(the first layer in the tuple is outermost). A recommended ordering:

```text
Request → [Fallback → [Retry → [Breaker → [Timeout → Operation]]]]
```

* **Fallback** (outermost): guarantees a usable response even if every retry is exhausted.
* **Retry**: retries the entire inner stack; each attempt gets its own timeout.
* **Breaker**: short-circuits failing calls so retry can back off until the breaker resets.
* **Timeout** (innermost): bounds each individual attempt.

Keep `Timeout` **inside** `Retry` so that a timed-out attempt is aborted and retried
correctly. If `Timeout` were outside, a single timeout would govern all attempts combined
and could cancel everything with no chance to recover.

## Tower Compatibility

All resilience middleware are compatible with the Tower ecosystem when the `tower-service`
feature is enabled. This allows you to use `tower::ServiceBuilder` to compose middleware stacks:

```rust
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use seatbelt::{RecoveryInfo, ResilienceContext};
use tower::ServiceBuilder;

let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock);

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
    .service_fn(|input: String| async move { Ok::<_, String>(format!("processed: {input}")) });
```

## Examples

Runnable examples covering each middleware and common composition patterns:

* [`timeout`][__link13]: Basic timeout that cancels long-running operations.
* [`timeout_advanced`][__link14]: Dynamic timeout durations and timeout callbacks.
* [`retry`][__link15]: Automatic retry with input cloning and recovery classification.
* [`retry_advanced`][__link16]: Custom input cloning with attempt metadata injection.
* [`retry_outage`][__link17]: Input restoration from errors when cloning is not possible.
* [`breaker`][__link18]: Circuit breaker that monitors failure rates.
* [`hedging`][__link19]: Hedging slow requests with parallel attempts to reduce tail latency.
* [`fallback`][__link20]: Substitutes default values for invalid outputs.
* [`resilience_pipeline`][__link21]: Composing retry and timeout with metrics.
* [`tower`][__link22]: Tower `ServiceBuilder` integration.
* [`config`][__link23]: Loading settings from a [JSON file][__link24].

## Features

This crate provides several optional features that can be enabled in your `Cargo.toml`:

* **`timeout`** - Enables the [`timeout`][__link25] middleware for canceling long-running operations.
* **`retry`** - Enables the [`retry`][__link26] middleware for automatically retrying failed operations with
  configurable backoff strategies, jitter, and recovery classification.
* **`hedging`** - Enables the [`hedging`][__link27] middleware for reducing tail latency via additional
  concurrent requests with configurable delay modes.
* **`breaker`** - Enables the [`breaker`][__link28] middleware for preventing cascading failures.
* **`fallback`** - Enables the [`fallback`][__link29] middleware for replacing invalid output with a
  user-defined alternative.
* **`metrics`** - Exposes the OpenTelemetry metrics API for collecting and reporting metrics.
* **`logs`** - Enables structured logging for resilience middleware using the `tracing` crate.
* **`serde`** - Enables `serde::Serialize` and `serde::Deserialize` implementations for
  configuration types.
* **`tower-service`** - Enables [`tower_service::Service`][__link30] trait implementations for all
  resilience middleware.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seatbelt">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG63RPvQRXuQSGzmDAHkjkSvqG1QA-4AlsJOaG2Tob1JFn2wuYWSFgmdsYXllcmVkZTAuMy4wgmtyZWNvdmVyYWJsZWUwLjEuMYJoc2VhdGJlbHRlMC4zLjGCZHRpY2tlMC4yLjGCbXRvd2VyX3NlcnZpY2VlMC4zLjM
 [__link0]: https://crates.io/crates/layered/0.3.0
 [__link1]: https://docs.rs/layered/0.3.0/layered/?search=Stack
 [__link10]: https://docs.rs/seatbelt/0.3.1/seatbelt/hedging/index.html
 [__link11]: https://docs.rs/seatbelt/0.3.1/seatbelt/breaker/index.html
 [__link12]: https://docs.rs/seatbelt/0.3.1/seatbelt/fallback/index.html
 [__link13]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/timeout.rs
 [__link14]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/timeout_advanced.rs
 [__link15]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry.rs
 [__link16]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry_advanced.rs
 [__link17]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry_outage.rs
 [__link18]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/breaker.rs
 [__link19]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/hedging.rs
 [__link2]: https://docs.rs/tick/0.2.1/tick/?search=Clock
 [__link20]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/fallback.rs
 [__link21]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/resilience_pipeline.rs
 [__link22]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/tower.rs
 [__link23]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/config.rs
 [__link24]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/config.json
 [__link25]: https://docs.rs/seatbelt/0.3.1/seatbelt/timeout/index.html
 [__link26]: https://docs.rs/seatbelt/0.3.1/seatbelt/retry/index.html
 [__link27]: https://docs.rs/seatbelt/0.3.1/seatbelt/hedging/index.html
 [__link28]: https://docs.rs/seatbelt/0.3.1/seatbelt/breaker/index.html
 [__link29]: https://docs.rs/seatbelt/0.3.1/seatbelt/fallback/index.html
 [__link3]: https://crates.io/crates/tick/0.2.1
 [__link30]: https://docs.rs/tower_service/0.3.3/tower_service/?search=Service
 [__link4]: https://docs.rs/seatbelt/0.3.1/seatbelt/?search=ResilienceContext
 [__link5]: https://docs.rs/seatbelt/0.3.1/seatbelt/?search=ResilienceContext
 [__link6]: https://docs.rs/recoverable/0.1.1/recoverable/?search=RecoveryInfo
 [__link7]: https://docs.rs/recoverable/0.1.1/recoverable/?search=Recovery
 [__link8]: https://docs.rs/seatbelt/0.3.1/seatbelt/timeout/index.html
 [__link9]: https://docs.rs/seatbelt/0.3.1/seatbelt/retry/index.html
