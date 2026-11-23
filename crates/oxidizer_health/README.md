<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Oxidizer Health Logo" width="96">

# Oxidizer Health

[![crate.io](https://img.shields.io/crates/v/oxidizer_health.svg)](https://crates.io/crates/oxidizer_health)
[![docs.rs](https://docs.rs/oxidizer_health/badge.svg)](https://docs.rs/oxidizer_health)
[![MSRV](https://img.shields.io/crates/msrv/oxidizer_health)](https://crates.io/crates/oxidizer_health)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Component health reports and HTTP readiness checks for Rust services.

A service reports the worst state among its components: `Healthy`, `Degraded`, or `Unhealthy`.
For example, if a database is degraded and a cache is healthy, the service is degraded.
Use the overall state for readiness checks and [component reports][__link0] to find the reason.

## Quick start

Register a component for the database. Use its [`Publisher`][__link1] to report slow queries:

```rust
use oxidizer_health::{Aggregator, Health};

let aggregator = Aggregator::new();
let database = aggregator.component("database");
let mut publisher = database.publisher();

publisher.degraded("slow queries");
assert_eq!(publisher.signal().state(), Health::Degraded);
```

Publishing updates the local signal immediately. [`Aggregator::state`][__link2] reads the latest
aggregate, so it can briefly report the previous state. A component without publishers is
`Unhealthy`, as is a service without components.

## Write a custom health check

Your application decides what to measure, when to measure it, and what counts as unhealthy.
A check is ordinary Rust code that publishes its result.
For example, a database client can count consecutive failures and reset the count after a
successful request. After each request, it passes the current count to this function:

```rust
use oxidizer_health::{Aggregator, Health, Publisher};

fn report_failures(publisher: &mut Publisher, failures: u32) {
    let state = match failures {
        0 => Health::Healthy,
        1..=2 => Health::Degraded,
        _ => Health::Unhealthy,
    };
    publisher.publish(state, [("consecutive_failures", i64::from(failures))]);
}

let aggregator = Aggregator::new();
let database = aggregator.component("database");
let mut publisher = database.publisher();

report_failures(&mut publisher, 3);
assert_eq!(publisher.signal().state(), Health::Unhealthy);

report_failures(&mut publisher, 0);
assert_eq!(publisher.signal().state(), Health::Healthy);
```

These thresholds are the client’s policy. A new publisher starts `Healthy`. Publishing
`Unhealthy` immediately afterward does not prevent a probe from seeing that initial `Healthy`.

If required dependencies must block readiness at startup, register their components on a new
[`Aggregator`][__link3] before creating any publishers or serving probes. Keep each component alive and
empty until its first successful check. An empty component is `Unhealthy`. After success,
create its publisher and reuse it for later failures and recoveries. Dropping the publisher
removes its contribution. Dropping the last [`Component`][__link4] handle unregisters the component.

## Diagnose a change

When readiness is false, a read-only [`Monitor`][__link5] can identify the component and show what its
publishers reported. This observer prints the component summary followed by each distinct problem:

```rust
use oxidizer_health::{Filter, Health, Monitor};

async fn show_problems(monitor: &Monitor) {
    for report in monitor.reports(Filter::DEGRADED | Filter::UNHEALTHY).await {
        if report.state() == Health::Healthy {
            continue;
        }
        println!("{report}");
        for state in [Health::Degraded, Health::Unhealthy] {
            for (signal, _) in report.signals(state) {
                println!("  {signal}");
            }
        }
    }
}
```

For the quick start, the report names `database` and the signal reads `Degraded (reason=slow queries)`.
The filter selects signals, not components. A component without publishers has an `Unhealthy`
summary but no signal to print. For continuous monitoring, read the reports, await
[`Monitor::changed`][__link6], then read again. Multiple updates can produce one notification.

## HTTP readiness

Health reporting works without HTTP. Enable `http` to use `http::service` with Tower,
or `request-handler` to use `http::request_handler` with `http_extensions`.
Both return HTTP 200 with JSON `true` for `Healthy` and HTTP 503 with JSON `false` otherwise.

## Additional documentation

* Register a [`Component`][__link7] with [`Aggregator::component`][__link8] and report changes with [`Publisher::publish`][__link9].
* Get a read-only [`Monitor`][__link10] with [`Aggregator::monitor`][__link11] to observe changes independently of publishers.
* Select diagnostic signals with [`Filter`][__link12] and inspect their attributes and publisher counts with [`Report::signals`][__link13].
* Serve readiness with one of the [HTTP adapters][__link14].


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/oxidizer_health">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbQAaznBUfBvQbcDNw_m5OW1Yb2LWTSpdyZSgb5wJ43Y7YvK1hZIGCb294aWRpemVyX2hlYWx0aGUwLjEuMA
 [__link0]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Report
 [__link1]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Publisher
 [__link10]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Monitor
 [__link11]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::monitor
 [__link12]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Filter
 [__link13]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Report::signals
 [__link14]: https://docs.rs/oxidizer_health/latest/oxidizer_health/http/
 [__link2]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::state
 [__link3]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator
 [__link4]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Component
 [__link5]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Monitor
 [__link6]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Monitor::changed
 [__link7]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Component
 [__link8]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::component
 [__link9]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Publisher::publish
