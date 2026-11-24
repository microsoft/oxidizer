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

Keep the component and its publishers alive while the subsystem participates in health reporting.
Dropping a publisher removes its contribution; dropping the last [`Component`][__link13] handle unregisters
the component. A component without publishers is `Unhealthy`, as is an [`Aggregator`][__link14] without components.

## Custom health checks and HTTP readiness

A custom check measures a dependency or application condition and publishes the result.
The application invokes the check at startup, periodically, or after relevant operations.
The HTTP helper reads the latest aggregate for each request.

This complete example checks TCP connectivity to a backend and invokes the Tower readiness
handler with a probe request. A local TCP listener supplies the backend so the example can run
independently. It uses the `http` feature and Tower with the `util` feature for `oneshot`.
Run the async code in an application executor.

```rust
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use http::{Request, StatusCode};
use oxidizer_health::http::service;
use oxidizer_health::{Aggregator, Health, Publisher};
use tower::ServiceExt;

fn check_backend(address: SocketAddr, publisher: &mut Publisher) {
    match TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
        Ok(_) => publisher.publish(Health::Healthy, [("endpoint", address.to_string())]),
        Err(error) => publisher.publish(Health::Unhealthy, [("error", error.to_string())]),
    }
}

let listener = TcpListener::bind("127.0.0.1:0").unwrap();
let address = listener.local_addr().unwrap();
let aggregator = Aggregator::new();
let backend = aggregator.component("backend");
let mut monitor = aggregator.monitor();

let mut publisher = backend.publisher();
check_backend(address, &mut publisher);
while monitor.state() != publisher.signal().state() {
    monitor.changed().await;
}

let handler = service(aggregator);
let request = Request::builder().uri("/health/ready").body(()).unwrap();
let response = handler.oneshot(request).await.unwrap();
assert_eq!(response.status(), StatusCode::OK);
assert_eq!(response.body(), "true");
```

The check measures TCP connectivity; an application-level check can instead perform a protocol
exchange or inspect application state. Reuse the publisher for subsequent checks so failures and
recoveries replace its previous signal.

Both HTTP helpers return HTTP 200 with JSON `true` for `Healthy`, and HTTP 503 with JSON `false`
for `Degraded` or `Unhealthy`. Responses use `Content-Type: application/json` and
`Cache-Control: no-store`. Register the handler on a readiness route in your HTTP server;
the example invokes it directly without starting a server.

A new publisher starts `Healthy`. For dependencies that must pass a check before admitting
traffic, register their components on a new [`Aggregator`][__link15] before serving probes. Keep each component
empty until its first successful check, then create and retain its publisher for later results.
This keeps the component `Unhealthy` during startup.

## Diagnose a change

A read-only [`Monitor`][__link16] provides component reports independently of publishers.
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

Reports group identical signals and count their publishers. [`Filter`][__link17] selects diagnostic
signals; all registered components and their aggregate states remain in the reports.
An empty component has an `Unhealthy` summary with no signals. For continuous observation,
read the reports, await [`Monitor::changed`][__link18], and repeat.

## Features

* `http` enables the [Tower readiness service][__link19].
* `request-handler` enables the [`http_extensions` readiness handler][__link20].

No features are enabled by default. Collection, aggregation, change notifications, and reports
are available in the default configuration.

## Additional documentation

* Register a [`Component`][__link21] with [`Aggregator::component`][__link22] and report changes with [`Publisher::publish`][__link23].
* Get a read-only [`Monitor`][__link24] with [`Aggregator::monitor`][__link25] to observe changes independently of publishers.
* Select diagnostic signals with [`Filter`][__link26] and inspect their attributes and publisher counts with [`Report::signals`][__link27].
* Serve readiness with one of the [HTTP adapters][__link28].


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/oxidizer_health">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbS6VIXSRo6gobgFQr_MUOVd0bbtCXAa3cRWsbExrSJXOEDyZhZIGCb294aWRpemVyX2hlYWx0aGUwLjEuMA
 [__link0]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Publisher
 [__link1]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Component
 [__link10]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::component
 [__link11]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Signal
 [__link12]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Monitor::changed
 [__link13]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Component
 [__link14]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator
 [__link15]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator
 [__link16]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Monitor
 [__link17]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Filter
 [__link18]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Monitor::changed
 [__link19]: https://docs.rs/oxidizer_health/latest/oxidizer_health/http/fn.service.html
 [__link2]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator
 [__link20]: https://docs.rs/oxidizer_health/latest/oxidizer_health/http/fn.request_handler.html
 [__link21]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Component
 [__link22]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::component
 [__link23]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Publisher::publish
 [__link24]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Monitor
 [__link25]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::monitor
 [__link26]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Filter
 [__link27]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Report::signals
 [__link28]: https://docs.rs/oxidizer_health/latest/oxidizer_health/http/
 [__link3]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Health::Healthy
 [__link4]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Health::Degraded
 [__link5]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Health::Unhealthy
 [__link6]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Report
 [__link7]: https://docs.rs/oxidizer_health/latest/oxidizer_health/http/
 [__link8]: https://docs.rs/http_extensions
 [__link9]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator
