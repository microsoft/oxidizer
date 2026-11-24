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

Health signal collection, aggregation, and HTTP readiness helpers for Rust services.

[`Publisher`][__link0]s collect application-defined health signals into named [`Component`][__link1]s.
Each component aggregates its publishers, and an [`Aggregator`][__link2] combines the components into
an overall service state: [`Healthy`][__link3], [`Degraded`][__link4], or
[`Unhealthy`][__link5]. At each level, the worst state wins.
[Reports][__link6] retain diagnostic attributes and publisher counts, while the optional
[HTTP helpers][__link7] expose readiness
through Tower or [`http_extensions`][__link8].

## Quick start

Create an [`Aggregator`][__link9], register an application-defined component, and publish a health signal.
[`Aggregator::component`][__link10] creates the named component on first use; subsequent calls with the
same name return a handle to that component. The name identifies a subsystem, not a built-in check.

```rust
use oxidizer_health::{Aggregator, Health};

let aggregator = Aggregator::new();
let backend = aggregator.component("backend");
let mut monitor = aggregator.monitor();
let mut publisher = backend.publisher();

publisher.degraded("connection pool exhausted");
while monitor.state() != Health::Degraded {
    monitor.changed().await;
}
assert_eq!(monitor.state(), Health::Degraded);
```

Publishing updates the local [`Signal`][__link11] immediately; aggregation runs in the background.
[`Monitor::changed`][__link12] notifies observers when state or report data may have changed.
Re-read the current state after each notification, as updates can coalesce.

Dropping a publisher removes its contribution; dropping the last [`Component`][__link13] handle unregisters
the component. A component without publishers is `Unhealthy`, as is an [`Aggregator`][__link14] without components.

## Diagnose a change

A read-only [`Monitor`][__link15] provides component reports independently of publishers.
This example publishes a problem, waits for aggregation, and prints its diagnostic signal:

```rust
use oxidizer_health::{Aggregator, Filter, Health};

let aggregator = Aggregator::new();
let backend = aggregator.component("backend");
let mut monitor = aggregator.monitor();
let mut publisher = backend.publisher();
publisher.degraded("connection pool exhausted");
while monitor.state() != Health::Degraded {
    monitor.changed().await;
}

for report in monitor.reports(Filter::DEGRADED | Filter::UNHEALTHY).await {
    println!("{report}");
    for state in [Health::Degraded, Health::Unhealthy] {
        for (signal, count) in report.signals(state) {
            println!("  {signal} ({count} publishers)");
        }
    }
}
```

Reports group identical signals and count their publishers. [`Filter`][__link16] selects diagnostic
signals; all registered components and their aggregate states remain in the reports.
An empty component has an `Unhealthy` summary with no signals. For continuous observation,
read the reports, await [`Monitor::changed`][__link17], and repeat.

## Features

* `http` enables the [Tower readiness service][__link18].
* `request-handler` enables the [`http_extensions` readiness handler][__link19].

No features are enabled by default. Collection, aggregation, change notifications, and reports
are available in the default configuration.

## Additional documentation

* Register a [`Component`][__link20] with [`Aggregator::component`][__link21] and report changes with [`Publisher::publish`][__link22].
* Get a read-only [`Monitor`][__link23] with [`Aggregator::monitor`][__link24] to observe changes independently of publishers.
* Select diagnostic signals with [`Filter`][__link25] and inspect their attributes and publisher counts with [`Report::signals`][__link26].
* Serve readiness with one of the [HTTP adapters][__link27].


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/oxidizer_health">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbNAapo5IuxXIbVKGk2lqf504bDsR8lgfooBYb0hxkCq5PSGVhZIGCb294aWRpemVyX2hlYWx0aGUwLjEuMA
 [__link0]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Publisher
 [__link1]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Component
 [__link10]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::component
 [__link11]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Signal
 [__link12]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Monitor::changed
 [__link13]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Component
 [__link14]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator
 [__link15]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Monitor
 [__link16]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Filter
 [__link17]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Monitor::changed
 [__link18]: https://docs.rs/oxidizer_health/latest/oxidizer_health/http/fn.service.html
 [__link19]: https://docs.rs/oxidizer_health/latest/oxidizer_health/http/fn.request_handler.html
 [__link2]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator
 [__link20]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Component
 [__link21]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::component
 [__link22]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Publisher::publish
 [__link23]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Monitor
 [__link24]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::monitor
 [__link25]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Filter
 [__link26]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Report::signals
 [__link27]: https://docs.rs/oxidizer_health/latest/oxidizer_health/http/
 [__link3]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Health::Healthy
 [__link4]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Health::Degraded
 [__link5]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Health::Unhealthy
 [__link6]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Report
 [__link7]: https://docs.rs/oxidizer_health/latest/oxidizer_health/http/
 [__link8]: https://docs.rs/http_extensions
 [__link9]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator
