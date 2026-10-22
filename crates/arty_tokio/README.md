<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Tokio Logo" width="96">

# Arty Tokio

[![crate.io](https://img.shields.io/crates/v/arty_tokio.svg)](https://crates.io/crates/arty_tokio)
[![docs.rs](https://docs.rs/arty_tokio/badge.svg)](https://docs.rs/arty_tokio)
[![MSRV](https://img.shields.io/crates/msrv/arty_tokio)](https://crates.io/crates/arty_tokio)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Tokio compatibility for tasks running on [`arty`][__link0].

Call [`init`][__link1] once at the start of an Arty task. It enters one Tokio runtime
context on every fixed Arty worker. It returns after all workers are ready.

## Default runtime

[`TokioHandle::default`][__link2] asks [`init`][__link3] to build one shared, multi-thread
Tokio runtime:

```rust
use arty::task::Builtins;
use arty_tokio::TokioHandle;

arty_tokio::init(&cx, TokioHandle::default()).await?;
let value = tokio::spawn(async { 42 })
    .await
    .expect("Tokio task must complete");
assert_eq!(value, 42);
Ok(())
```

The crate owns this runtime until Arty stops. It uses
[`tokio::runtime::Builder::new_multi_thread`][__link4] and Tokio’s default worker
count. It also calls [`tokio::runtime::Builder::enable_all`][__link5].

## Initialization failure

Initialization prepares every worker before it commits. If a worker fails,
[`init`][__link6] removes each Tokio context that it installed. Canceling [`init`][__link7]
also starts this cleanup. A later call can retry on the same Arty runtime
after cleanup completes.

## External runtime

Pass a multi-thread Tokio [`Handle`][__link8] to use a Tokio
runtime that your application owns:

```rust
use arty::task::Builtins;
use tokio::runtime::Builder;

let tokio = Builder::new_multi_thread().enable_all().build()?;
arty_tokio::init(&cx, tokio.handle().clone()).await?;
let value = tokio::spawn(async { 42 })
    .await
    .expect("Tokio task must complete");
assert_eq!(value, 42);
```

The crate stores only the handle. It does not own the supplied runtime. Keep
that runtime alive while Arty uses Tokio. [`init`][__link9] rejects current-thread
Tokio runtimes.

## Compatibility

After successful initialization, normal Arty task execution on every worker
supports:

* [`tokio::runtime::Handle::current`][__link10] and
  [`tokio::runtime::Handle::try_current`][__link11];
* [`tokio::spawn`][__link12] and [`tokio::task::spawn_blocking`][__link13] submission;
* Tokio timers, networking, and I/O resources with enabled drivers;
* directly awaiting Tokio-based futures from Arty tasks.

Tokio tasks run on Tokio workers. They can move between Tokio workers. They
do not keep Arty thread-aware state or receive [`arty::task::Builtins`][__link14].
They have no affinity to the Arty worker that submitted them. Tokio
[`JoinHandle`][__link15] and
[`JoinError`][__link16] behavior does not change.

The crate does not support these APIs or behaviors:

* current-thread Tokio runtimes and implicit `LocalSet` / `spawn_local`
  integration;
* [`tokio::task::block_in_place`][__link17] from an Arty worker;
* `Runtime::block_on` or `Handle::block_on` from an Arty worker;
* Tokio cooperative budgets or Tokio task-local executor context when Arty
  directly polls a Tokio future. Run the library root inside
  [`tokio::spawn`][__link18];
* Tokio context during final Arty timer or worker teardown callbacks.

## Shutdown

Installer tasks remain parked until Arty stops. Arty drops each installer on
its worker. The installer leaves the Tokio context before the crate drops an
internally owned runtime. Tokio waits for blocking work to finish. Long
[`tokio::task::spawn_blocking`][__link19] work can delay Arty shutdown.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_tokio">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbnNcsDLsVgGIbSGNd95cNKSAbokCwR4vbhLAb8lJ6jhAbY6thZIOCZGFydHllMC41LjCCamFydHlfdG9raW9lMC4xLjCCZXRva2lvZjEuNTMuMQ
 [__link0]: https://crates.io/crates/arty/0.5.0
 [__link1]: https://docs.rs/arty_tokio/0.1.0/arty_tokio/fn.init.html
 [__link10]: https://docs.rs/tokio/1.53.1/tokio/?search=runtime::Handle::current
 [__link11]: https://docs.rs/tokio/1.53.1/tokio/?search=runtime::Handle::try_current
 [__link12]: https://docs.rs/tokio/1.53.1/tokio/?search=spawn
 [__link13]: https://docs.rs/tokio/1.53.1/tokio/?search=task::spawn_blocking
 [__link14]: https://docs.rs/arty/0.5.0/arty/?search=task::Builtins
 [__link15]: https://docs.rs/tokio/1.53.1/tokio/?search=task::JoinHandle
 [__link16]: https://docs.rs/tokio/1.53.1/tokio/?search=task::JoinError
 [__link17]: https://docs.rs/tokio/1.53.1/tokio/?search=task::block_in_place
 [__link18]: https://docs.rs/tokio/1.53.1/tokio/?search=spawn
 [__link19]: https://docs.rs/tokio/1.53.1/tokio/?search=task::spawn_blocking
 [__link2]: https://docs.rs/arty_tokio/0.1.0/arty_tokio/?search=TokioHandle::default
 [__link3]: https://docs.rs/arty_tokio/0.1.0/arty_tokio/fn.init.html
 [__link4]: https://docs.rs/tokio/1.53.1/tokio/?search=runtime::Builder::new_multi_thread
 [__link5]: https://docs.rs/tokio/1.53.1/tokio/?search=runtime::Builder::enable_all
 [__link6]: https://docs.rs/arty_tokio/0.1.0/arty_tokio/fn.init.html
 [__link7]: https://docs.rs/arty_tokio/0.1.0/arty_tokio/fn.init.html
 [__link8]: https://docs.rs/tokio/1.53.1/tokio/?search=runtime::Handle
 [__link9]: https://docs.rs/arty_tokio/0.1.0/arty_tokio/fn.init.html
