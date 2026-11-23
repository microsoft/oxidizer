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

Application health aggregation.

Application code owns [`Publisher`][__link0]s beside the workers or clients that detect health
changes. Each publisher publishes a [`Signal`][__link1] containing a [`Health`][__link2] state and
[`Attribute`][__link3]s. A [`Component`][__link4] aggregates the publishers of one subsystem, and the
[`Aggregator`][__link5] exposes process health and component [`Report`][__link6]s.

Aggregation is worst-state-wins at both levels. A component without publishers and an
aggregator without components are `Unhealthy`.

[`state`][__link7] is a cheap snapshot read for readiness decisions.
[`changed`][__link8] is an edge notification: it resolves after the state or report
data may have changed, after which consumers read the current state. Publishing never waits for
a consumer.
If the aggregation worker cannot run, state reads return `Unhealthy` and the failure is
reported through diagnostics.

## Examples

```rust
use oxidizer_health::{Aggregator, Attribute, Filter, Health};

let mut aggregator = Aggregator::new();

let startup = aggregator.component("startup");
let database = aggregator.component("database");

let mut startup_publisher = startup.publisher();
let mut primary_publisher = database.publisher();
let mut replica_publisher = database.publisher();

startup_publisher.publish(Health::Unhealthy, [("phase", "starting")]);
primary_publisher.degraded("High latency detected");
replica_publisher.publish(Health::Healthy, Vec::<Attribute>::new());

startup_publisher.publish(Health::Healthy, Vec::<Attribute>::new());
while aggregator.state() != Health::Degraded {
    aggregator.changed().await;
}

for report in aggregator
    .reports(Filter::DEGRADED | Filter::UNHEALTHY)
    .await
{
    for (signal, publishers) in report.signals(Health::Degraded) {
        println!("{report}: {signal} from {publishers} publishers");
    }
}
```

## Features

* `http`: `http::service`, a Tower readiness service.
* `request-handler`: `http::request_handler`, an `http_extensions` readiness request
  handler.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/oxidizer_health">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbJml-Qsqtun4bIbUZo_lts68bddJCu_ULdwQb0v6yJkdVT1lhZIGCb294aWRpemVyX2hlYWx0aGUwLjEuMA
 [__link0]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Publisher
 [__link1]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Signal
 [__link2]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Health
 [__link3]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Attribute
 [__link4]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Component
 [__link5]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator
 [__link6]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Report
 [__link7]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::state
 [__link8]: https://docs.rs/oxidizer_health/0.1.0/oxidizer_health/?search=Aggregator::changed
