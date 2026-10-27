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

Lets [`arty`][__link0] tasks use Tokio-based libraries.

Call [`init`][__link1] from an Arty task during application startup. It enters the
same Tokio runtime context on every fixed Arty worker and returns when all
workers are ready.

## Default runtime

Pass [`TokioHandle::default`][__link2] to build one multi-thread Tokio runtime shared
by all Arty workers:

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

This crate owns the runtime until Arty stops. It uses
[`tokio::runtime::Builder::new_multi_thread`][__link3] and Tokio’s default worker
count, with all Tokio drivers enabled.

## Using a Tokio-based library

After initialization, an Arty task can await async code that requires a
Tokio runtime context:

```rust
use std::time::Duration;

use arty::task::Builtins;
use arty_tokio::TokioHandle;

async fn call_tokio_based_library() -> u64 {
    tokio::time::sleep(Duration::from_millis(1)).await;
    42
}

arty_tokio::init(&cx, TokioHandle::default()).await?;
assert_eq!(call_tokio_based_library().await, 42);
```

The complete runnable version is in the `tokio_library` example:
`cargo run -p arty_tokio --example tokio_library`.

## Initialization failure

Initialization prepares every worker before it commits. If any worker
fails, [`init`][__link4] removes the contexts created by that attempt. Canceling
[`init`][__link5] starts the same cleanup. A later call can retry after cleanup
finishes.

## External runtime

Pass a multi-thread Tokio [`Handle`][__link6] to use a runtime owned by the
application:

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

This crate stores only a clone of the handle. Keep the runtime alive while
Arty uses Tokio. [`init`][__link7] rejects current-thread Tokio runtimes.

## Compatibility

After initialization, Arty tasks on every worker can use:

* [`tokio::runtime::Handle::current`][__link8] and
  [`tokio::runtime::Handle::try_current`][__link9];
* [`tokio::spawn`][__link10] and [`tokio::task::spawn_blocking`][__link11] submission;
* Tokio timers, networking, and I/O resources with enabled drivers;
* directly awaiting Tokio-based futures from Arty tasks.

Tokio tasks run on Tokio workers and can move between them. They do not
receive [`arty::task::Builtins`][__link12] or keep Arty thread-aware state. They are
not tied to the Arty worker that submitted them. Tokio
[`JoinHandle`][__link13] and
[`JoinError`][__link14] keep their normal behavior.

This crate does not support:

* current-thread Tokio runtimes and implicit `LocalSet` / `spawn_local`
  integration;
* [`tokio::task::block_in_place`][__link15] from an Arty worker;
* `Runtime::block_on` or `Handle::block_on` from an Arty worker;
* Tokio cooperative scheduling or Tokio task-local context when Arty polls
  a Tokio future directly. If a library needs them, run its root future
  with [`tokio::spawn`][__link16];
* Tokio context during final Arty timer or worker teardown callbacks.

## Shutdown

Installer tasks stay parked until Arty stops. Arty drops each installer on
its worker, which leaves the Tokio context. If this crate owns the runtime,
it then drops the runtime. Tokio waits for blocking work to finish during
runtime drop, so long [`tokio::task::spawn_blocking`][__link17] work can delay Arty
shutdown.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_tokio">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbOS9Jw_H3N2gbTHPdSqUzyS4b1J-hoXs5FiUbZ_JLO_jS2bRhZIOCZGFydHllMC41LjCCamFydHlfdG9raW9lMC4xLjCCZXRva2lvZjEuNTMuMw
 [__link0]: https://crates.io/crates/arty/0.5.0
 [__link1]: https://docs.rs/arty_tokio/0.1.0/arty_tokio/fn.init.html
 [__link10]: https://docs.rs/tokio/1.53.3/tokio/?search=spawn
 [__link11]: https://docs.rs/tokio/1.53.3/tokio/?search=task::spawn_blocking
 [__link12]: https://docs.rs/arty/0.5.0/arty/?search=task::Builtins
 [__link13]: https://docs.rs/tokio/1.53.3/tokio/?search=task::JoinHandle
 [__link14]: https://docs.rs/tokio/1.53.3/tokio/?search=task::JoinError
 [__link15]: https://docs.rs/tokio/1.53.3/tokio/?search=task::block_in_place
 [__link16]: https://docs.rs/tokio/1.53.3/tokio/?search=spawn
 [__link17]: https://docs.rs/tokio/1.53.3/tokio/?search=task::spawn_blocking
 [__link2]: https://docs.rs/arty_tokio/0.1.0/arty_tokio/?search=TokioHandle::default
 [__link3]: https://docs.rs/tokio/1.53.3/tokio/?search=runtime::Builder::new_multi_thread
 [__link4]: https://docs.rs/arty_tokio/0.1.0/arty_tokio/fn.init.html
 [__link5]: https://docs.rs/arty_tokio/0.1.0/arty_tokio/fn.init.html
 [__link6]: https://docs.rs/tokio/1.53.3/tokio/?search=runtime::Handle
 [__link7]: https://docs.rs/arty_tokio/0.1.0/arty_tokio/fn.init.html
 [__link8]: https://docs.rs/tokio/1.53.3/tokio/?search=runtime::Handle::current
 [__link9]: https://docs.rs/tokio/1.53.3/tokio/?search=runtime::Handle::try_current
