<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Performables Logo" width="96">

# Performables

[![crate.io](https://img.shields.io/crates/v/performables.svg)](https://crates.io/crates/performables)
[![docs.rs](https://docs.rs/performables/badge.svg)](https://docs.rs/performables)
[![MSRV](https://img.shields.io/crates/msrv/performables)](https://crates.io/crates/performables)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Shared ownership and synchronization with thread-aware runtime and
`seismograph` integration.

`performables` provides [`Arc`][__link0], mutex and reader-writer locks,
barriers, condition variables, one-time initialization, and channels.
Synchronization supports blocking callers and executor-independent async
waits.

## Why use this instead of `std` or `tokio`?

The crate offers:

* **Uncontended performance.** Optimized for uncontended access, which is
  common in thread-per-core environments.
* **`seismograph` telemetry.** The optional `seismograph` feature records
  ownership, lock contention, and channel activity through Seismograph’s
  recorder. Normal operations supply the events without manual
  instrumentation at each call site.
* **Thread-aware ownership.** `Arc` with [`PerThread`][__link1] or
  [`PerNuma`][__link2] implements
  [`ThreadAware`][__link3] when its pointee is
  `Send + Sync`. A thread-aware runtime can relocate the pointer to a value
  local to the destination thread or NUMA node, rather than sharing one
  process-wide value as `std::sync::Arc` does.

See [`arc`][__link4] and [`sync`][__link5] for the available types and their full APIs.

## Live monitor integration workload

Run `cargo +1.95.0 test -p performables --features seismograph --test seismograph_monitor live_monitor -- --ignored --nocapture`
in one terminal, then `cargo +1.95.0 run -p seismograph_cli -- monitor`
in another terminal using the same checkout and OS environment.
Select `performables-integration`,
press Enter to connect, and press `s` to capture. The Primitives and Threads
tabs show real mutex and Arc activity from three named worker threads.
Press Enter in the workload terminal to stop it and remove its endpoint.
No administrator setup or application changes are required.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/performables">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb5nVHt1dSna4bHfOGWaJtq14bHOIGlWKWIO0bQioiH0BVu5ZhZIKCbHBlcmZvcm1hYmxlc2UwLjIuMIJxdGhyZWFkX2F3YXJlX2NvcmVlMC4xLjE
 [__link0]: https://docs.rs/performables/0.2.0/performables/?search=arc::Arc
 [__link1]: https://docs.rs/performables/0.2.0/performables/?search=arc::PerThread
 [__link2]: https://docs.rs/performables/0.2.0/performables/?search=arc::PerNuma
 [__link3]: https://docs.rs/thread_aware_core/0.1.1/thread_aware_core/?search=ThreadAware
 [__link4]: https://docs.rs/performables/0.2.0/performables/arc/index.html
 [__link5]: https://docs.rs/performables/0.2.0/performables/sync/index.html
