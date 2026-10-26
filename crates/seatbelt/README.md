<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Seatbelt Logo" width="96">

# Seatbelt

[![crate.io](https://img.shields.io/crates/v/seatbelt.svg)](https://crates.io/crates/seatbelt)
[![docs.rs](https://docs.rs/seatbelt/badge.svg)](https://docs.rs/seatbelt)
[![MSRV](https://img.shields.io/crates/msrv/seatbelt)](https://crates.io/crates/seatbelt)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Resilience middleware for operations that can fail.

## Quick start

Use retry and timeout middleware to protect a network operation.
Retry repeats an operation after a temporary failure.
Timeout stops an operation when it takes too long.

```rust
use layered::{Execute, Service, Stack};
use seatbelt::retry::Retry;
use seatbelt::timeout::Timeout;
use seatbelt::{RecoveryInfo, ResilienceContext};

let context = ResilienceContext::new(&clock);
let service = (
    // Retry temporary failures.
    Retry::layer("retry", &context)
        .clone_input()
        .recovery_with(|output: &String, _| match output.as_str() {
            "temporary_error" => RecoveryInfo::retry(),
            "operation timed out" => RecoveryInfo::retry(),
            _ => RecoveryInfo::never(),
        }),
    // Apply the time limit to each retry attempt.
    Timeout::layer("timeout", &context)
        .timeout_output(|_| "operation timed out".to_string())
        .timeout(Duration::from_secs(30)),
    // Run the operation.
    Execute::new(my_string_operation),
)
    .into_service();

let result = service.execute("input data".to_string()).await;
```

## Purpose

Network operations can fail for a short time.
A server can restart, a connection can close, or a response can take too long.
Seatbelt provides middleware that handles these failures in a consistent way.
It also provides metrics and structured logs for resilience events.
The middleware does not depend on a specific async runtime.

## Overview

This crate uses [`layered`][__link0] to combine middleware.
Put the layers in a tuple, then use [`Stack`][__link1] to make a service.

The middleware uses [`Clock`][__link2] from [`tick`][__link3] for delays and timeouts.
Pass the clock to each layer through [`ResilienceContext`][__link4].

### Core types

* [`ResilienceContext`][__link5] holds the shared clock and telemetry configuration.
* [`RecoveryInfo`][__link6] states if an output permits another attempt.
* [`Recovery`][__link7] lets an output type provide its own recovery classification.

### Middleware

* [`timeout`][__link8] stops an operation after a time limit.
* [`retry`][__link9] repeats an operation after a recoverable output.
* [`hedging`][__link10] starts more attempts when an operation is slow.
* [`breaker`][__link11] blocks calls to a service that is failing.
* [`fallback`][__link12] replaces an invalid output with another value.

### Chaos testing

The [`chaos`][__link13] module adds controlled faults to a service pipeline.
Use it to test how the pipeline responds to failures.

* [`chaos::injection`][__link14] replaces service output at a configured rate.
* [`chaos::latency`][__link15] adds a delay before the inner service call.

## Middleware order

Layers run from the outside to the inside.
The first layer in the tuple is the outer layer.
Use this order for a common service pipeline:

```text
Request → [Fallback → [Retry → [Breaker → [Timeout → Operation]]]]
```

* `Fallback` supplies a value after the other layers cannot recover.
* `Retry` repeats the inner stack.
* `Breaker` blocks calls while the service is unhealthy.
* `Timeout` limits one attempt.

Put `Timeout` inside `Retry`.
Each retry attempt then gets its own time limit.
If `Timeout` is outside, one time limit covers all attempts.

## Tower compatibility

Enable `tower-service` to use Seatbelt middleware with Tower.
You can then use `tower::ServiceBuilder` to combine the layers.

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

* [`timeout`][__link16] limits the duration of an operation.
* [`timeout_advanced`][__link17] uses dynamic durations and callbacks.
* [`retry`][__link18] retries recoverable output.
* [`retry_advanced`][__link19] clones input and adds attempt data.
* [`retry_outage`][__link20] restores input from an error.
* [`breaker`][__link21] opens a circuit after repeated failures.
* [`hedging`][__link22] starts parallel attempts for slow operations.
* [`fallback`][__link23] replaces invalid output.
* [`resilience_pipeline`][__link24] combines retry, timeout, and metrics.
* [`tower`][__link25] uses Tower `ServiceBuilder`.
* [`config`][__link26] loads a [JSON file][__link27].
* [`chaos_injection`][__link28] replaces output at a configured rate.
* [`chaos_injection_advanced`][__link29] simulates a longer outage.
* [`chaos_latency`][__link30] adds artificial delay.

## Features

Enable the features that the service needs:

* `timeout` enables [`timeout`][__link31].
* `retry` enables [`retry`][__link32].
* `hedging` enables [`hedging`][__link33].
* `breaker` enables [`breaker`][__link34].
* `fallback` enables [`fallback`][__link35].
* `chaos-injection` enables [`chaos::injection`][__link36].
* `chaos-latency` enables [`chaos::latency`][__link37].
* `metrics` enables the OpenTelemetry metrics API.
* `logs` enables structured logs through `tracing`.
* `serde` adds serialization for configuration types.
* `tower-service` adds [`tower_service::Service`][__link38] implementations.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seatbelt">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb7WqNjwQtD-QbvaVVSN7eg28b7hfMEtlLHZ0baahRf6tsJIJhZIWCZ2xheWVyZWRlMC4zLjiCa3JlY292ZXJhYmxlZTAuMy4wgmhzZWF0YmVsdGYwLjExLjGCZHRpY2tlMC44LjGDbXRvd2VyLXNlcnZpY2VlMC4zLjNtdG93ZXJfc2VydmljZQ
 [__link0]: https://crates.io/crates/layered/0.3.8
 [__link1]: https://docs.rs/layered/0.3.8/layered/?search=Stack
 [__link10]: https://docs.rs/seatbelt/0.11.1/seatbelt/hedging/index.html
 [__link11]: https://docs.rs/seatbelt/0.11.1/seatbelt/breaker/index.html
 [__link12]: https://docs.rs/seatbelt/0.11.1/seatbelt/fallback/index.html
 [__link13]: https://docs.rs/seatbelt/0.11.1/seatbelt/chaos/index.html
 [__link14]: https://docs.rs/seatbelt/0.11.1/seatbelt/?search=chaos::injection
 [__link15]: https://docs.rs/seatbelt/0.11.1/seatbelt/?search=chaos::latency
 [__link16]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/timeout.rs
 [__link17]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/timeout_advanced.rs
 [__link18]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry.rs
 [__link19]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry_advanced.rs
 [__link2]: https://docs.rs/tick/0.8.1/tick/?search=Clock
 [__link20]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry_outage.rs
 [__link21]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/breaker.rs
 [__link22]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/hedging.rs
 [__link23]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/fallback.rs
 [__link24]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/resilience_pipeline.rs
 [__link25]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/tower.rs
 [__link26]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/config.rs
 [__link27]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/config.json
 [__link28]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/chaos_injection.rs
 [__link29]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/chaos_injection_advanced.rs
 [__link3]: https://crates.io/crates/tick/0.8.1
 [__link30]: https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/chaos_latency.rs
 [__link31]: https://docs.rs/seatbelt/0.11.1/seatbelt/timeout/index.html
 [__link32]: https://docs.rs/seatbelt/0.11.1/seatbelt/retry/index.html
 [__link33]: https://docs.rs/seatbelt/0.11.1/seatbelt/hedging/index.html
 [__link34]: https://docs.rs/seatbelt/0.11.1/seatbelt/breaker/index.html
 [__link35]: https://docs.rs/seatbelt/0.11.1/seatbelt/fallback/index.html
 [__link36]: https://docs.rs/seatbelt/0.11.1/seatbelt/?search=chaos::injection
 [__link37]: https://docs.rs/seatbelt/0.11.1/seatbelt/?search=chaos::latency
 [__link38]: https://docs.rs/tower-service/0.3.3/tower_service/?search=Service
 [__link4]: https://docs.rs/seatbelt/0.11.1/seatbelt/?search=ResilienceContext
 [__link5]: https://docs.rs/seatbelt/0.11.1/seatbelt/?search=ResilienceContext
 [__link6]: https://docs.rs/recoverable/0.3.0/recoverable/?search=RecoveryInfo
 [__link7]: https://docs.rs/recoverable/0.3.0/recoverable/?search=Recovery
 [__link8]: https://docs.rs/seatbelt/0.11.1/seatbelt/timeout/index.html
 [__link9]: https://docs.rs/seatbelt/0.11.1/seatbelt/retry/index.html
