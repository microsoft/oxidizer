<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Process Health Logo" width="96">

# Process Health

[![crate.io](https://img.shields.io/crates/v/process_health.svg)](https://crates.io/crates/process_health)
[![docs.rs](https://docs.rs/process_health/badge.svg)](https://docs.rs/process_health)
[![MSRV](https://img.shields.io/crates/msrv/process_health)](https://crates.io/crates/process_health)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Health signal collection and aggregation for Rust services.

[`Publisher`][__link0]s collect application-defined health signals into named [`Component`][__link1]s.
Each component aggregates its publishers, and an [`Aggregator`][__link2] combines the components into
an overall service state: [`Healthy`][__link3], [`Degraded`][__link4], or
[`Unhealthy`][__link5]. At each level, the worst state wins.
[Reports][__link6] retain diagnostic attributes and publisher counts.

## Quick start

Create an [`Aggregator`][__link7], register an application-defined component, and publish a health signal.
[`Aggregator::component`][__link8] creates the named component on first use; subsequent calls with the
same name return a handle to that component. The name identifies a subsystem, not a built-in check.

```rust
use health::{Aggregator, Health};
use observed::Sink;

let aggregator = Aggregator::new(Sink::noop());
let backend = aggregator.component("backend");
let mut monitor = aggregator.monitor();
let mut publisher = backend.publisher();

publisher.degraded("connection pool exhausted");
while monitor.state() != Health::Degraded {
    monitor.changed().await;
}
assert_eq!(monitor.state(), Health::Degraded);
```

Publishing updates the local [`Signal`][__link9] immediately. Aggregation runs in the background.
Several updates can produce one [`Monitor::changed`][__link10] notification. Read the latest state
or reports after each notification rather than counting transitions.

The loop waits for `Degraded`. If the worker stops or fails to start, readiness is `Unhealthy`.
A monitor created after that receives no notification.
If a wait must end even when the target state is unreachable, use a timeout or cancellation.

Dropping a publisher removes its contribution. A component unregisters after its last
[`Component`][__link11] handle and publisher drop. A component without publishers is `Unhealthy`, as is
an [`Aggregator`][__link12] without components.

## Diagnose a change

A read-only [`Monitor`][__link13] provides component reports independently of publishers.
This example publishes a problem, waits for aggregation, and prints its diagnostic signal:

```rust
use health::{Aggregator, Health};
use observed::Sink;

let aggregator = Aggregator::new(Sink::noop());
let backend = aggregator.component("backend");
let mut monitor = aggregator.monitor();
let mut publisher = backend.publisher();
publisher.degraded("connection pool exhausted");
while monitor.state() != Health::Degraded {
    monitor.changed().await;
}

for report in monitor.reports().await {
    println!("{report}");
    for state in [Health::Degraded, Health::Unhealthy] {
        for (signal, count) in report.signals(state) {
            println!("  {signal} ({count} publishers)");
        }
    }
}
```

Reports group identical signals and count their publishers. Call [`Report::signals`][__link14] for the
severity that the diagnostic consumer needs.
An empty component has an `Unhealthy` summary with no signals. For continuous observation,
read the reports, await [`Monitor::changed`][__link15], and repeat.

## Additional documentation

* Register a [`Component`][__link16] with [`Aggregator::component`][__link17] and report changes with [`Publisher::publish`][__link18].
* Get a read-only [`Monitor`][__link19] with [`Aggregator::monitor`][__link20] to observe changes independently of publishers.
* Select diagnostic signals by [`Health`][__link21] and inspect their attributes and publisher counts with [`Report::signals`][__link22].


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/process_health">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbX_4VzDhABZgbJ8ZXfu_ETgEbKOKR4xiqBGIbY88hYsWFXpZhZIGCbnByb2Nlc3NfaGVhbHRoZTAuMS4w
 [__link0]: https://docs.rs/process_health/0.1.0/process_health/?search=Publisher
 [__link1]: https://docs.rs/process_health/0.1.0/process_health/?search=Component
 [__link10]: https://docs.rs/process_health/0.1.0/process_health/?search=Monitor::changed
 [__link11]: https://docs.rs/process_health/0.1.0/process_health/?search=Component
 [__link12]: https://docs.rs/process_health/0.1.0/process_health/?search=Aggregator
 [__link13]: https://docs.rs/process_health/0.1.0/process_health/?search=Monitor
 [__link14]: https://docs.rs/process_health/0.1.0/process_health/?search=Report::signals
 [__link15]: https://docs.rs/process_health/0.1.0/process_health/?search=Monitor::changed
 [__link16]: https://docs.rs/process_health/0.1.0/process_health/?search=Component
 [__link17]: https://docs.rs/process_health/0.1.0/process_health/?search=Aggregator::component
 [__link18]: https://docs.rs/process_health/0.1.0/process_health/?search=Publisher::publish
 [__link19]: https://docs.rs/process_health/0.1.0/process_health/?search=Monitor
 [__link2]: https://docs.rs/process_health/0.1.0/process_health/?search=Aggregator
 [__link20]: https://docs.rs/process_health/0.1.0/process_health/?search=Aggregator::monitor
 [__link21]: https://docs.rs/process_health/0.1.0/process_health/?search=Health
 [__link22]: https://docs.rs/process_health/0.1.0/process_health/?search=Report::signals
 [__link3]: https://docs.rs/process_health/0.1.0/process_health/?search=Health::Healthy
 [__link4]: https://docs.rs/process_health/0.1.0/process_health/?search=Health::Degraded
 [__link5]: https://docs.rs/process_health/0.1.0/process_health/?search=Health::Unhealthy
 [__link6]: https://docs.rs/process_health/0.1.0/process_health/?search=Report
 [__link7]: https://docs.rs/process_health/0.1.0/process_health/?search=Aggregator
 [__link8]: https://docs.rs/process_health/0.1.0/process_health/?search=Aggregator::component
 [__link9]: https://docs.rs/process_health/0.1.0/process_health/?search=Signal
