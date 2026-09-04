<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Performables Logo" width="96">

# Performables

[![crate.io](https://img.shields.io/crates/v/performables.svg)](https://crates.io/crates/performables)
[![docs.rs](https://docs.rs/performables/badge.svg)](https://docs.rs/performables)
[![MSRV](https://img.shields.io/crates/msrv/performables)](https://crates.io/crates/performables)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Performance-oriented ownership and asynchronous synchronization primitives.

The synchronization types are executor-independent and optimize for the
uncontended case. Acquiring an available lock performs only atomic
operations; waiter allocation and queue locking occur after contention is
observed. Locks support asynchronous acquisition as well as explicit
`lock_sync`, `read_sync`, and `write_sync` operations for synchronous call
sites. Default lock acquisition panics on poison, while explicit `*_result`
APIs return [`sync::PoisonError`][__link0] with the acquired guard for recovery.
[`sync::barrier::Barrier`][__link1] and [`sync::condition::Condvar`][__link2] provide
asynchronous and blocking waits, while [`sync::once::OnceLock`][__link3] and
[`sync::once::LazyLock`][__link4] instrument one-time initialization.
[`sync::channel`][__link5] provides multi-producer queues, oneshot transfer, and
independently versioned latest-value observation. The default `seismograph`
feature enables runtime ownership and synchronization telemetry.

[`arc::Arc`][__link6] defaults to a process-wide allocation with the same
representation size as [`std::sync::Arc`][__link7]. Its thread-aware per-core and
per-NUMA strategies lazily materialize and reuse affinity-local values.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/performables">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEG9dVcQv7gDzkG7VJ-FsdvgXwG4ndzbdWNuz6G6a5_GehYxcvYXKEGzLtA9EtpaABGzhygmK3kZAMG9eziizuCMKCGyVd_bkn6prXYWSBgmxwZXJmb3JtYWJsZXNlMC4xLjA
 [__link0]: https://docs.rs/performables/0.1.0/performables/?search=sync::PoisonError
 [__link1]: https://docs.rs/performables/0.1.0/performables/?search=sync::barrier::Barrier
 [__link2]: https://docs.rs/performables/0.1.0/performables/?search=sync::condition::Condvar
 [__link3]: https://docs.rs/performables/0.1.0/performables/?search=sync::once::OnceLock
 [__link4]: https://docs.rs/performables/0.1.0/performables/?search=sync::once::LazyLock
 [__link5]: https://docs.rs/performables/0.1.0/performables/?search=sync::channel
 [__link6]: https://docs.rs/performables/0.1.0/performables/?search=arc::Arc
 [__link7]: https://doc.rust-lang.org/stable/std/?search=sync::Arc
