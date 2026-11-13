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

Use [`TokioCompatExt::tokio_compat`][__link1] from an async main function to create a
Tokio runtime and enter its context on every live Arty worker:

```rust
use arty::task::Context;
use arty_tokio::TokioCompatExt as _;

cx.control().tokio_compat();
let value = tokio::spawn(async { 42 }).await?;
assert_eq!(value, 42);
```

Use [`TokioCompatExt::tokio_compat_with`][__link2] to configure workers before the
runtime starts:

```rust
use arty::runtime::{Runtime, WorkersPolicy};
use arty_tokio::TokioCompatExt as _;
use tokio::runtime::Builder;

let tokio = Builder::new_multi_thread().enable_all().build()?;
let runtime = Runtime::builder()
    .workers(WorkersPolicy::exactly(2))
    .tokio_compat_with(tokio.handle())
    .build()?;
assert_eq!(runtime.scheduler().block_on(async |_| 42)?, 42);
runtime.stop()?;
```

The first initialization wins on each worker. Later calls to
[`TokioCompatExt::tokio_compat_with`][__link3] do nothing on that worker, even with a
different Tokio runtime. They do not replace an existing Tokio context.
Builder registrations run in registration order. Through runtime control,
setup runs immediately on the calling worker and is queued on other workers.

If you supply a Tokio handle, the application owns its runtime. Keep that
runtime alive while Arty tasks use Tokio. Each Arty worker stores a clone of
the handle.

If you supply an owned runtime or use [`TokioHandle::default`][__link4], Arty drops
the owned Tokio runtime when Arty shuts down.

For a current-thread Tokio runtime, the application must also run
[`Runtime::block_on`][__link5] on another thread while Arty tasks use Tokio.
Keeping that runtime alive or entering its handle does not drive its tasks,
timers, or I/O. Without `Runtime::block_on`, those operations can stall.

After setup, Arty tasks can use Tokio task spawning and libraries that
require a Tokio runtime context. Timers require the Tokio `time` feature and
an enabled time driver. Networking and I/O resources require the applicable
Tokio features and an enabled I/O driver. Configure these drivers when you
build an explicit Tokio runtime.
Tokio tasks run on Tokio workers. They do not receive
[`arty::task::Context`][__link6] or keep Arty thread-aware state.

Do not call `tokio::task::block_in_place`, `Runtime::block_on`, or
`Handle::block_on` from an Arty worker. `LocalSet` and `spawn_local` are not
integrated with Arty.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_tokio">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbhePaY4dAE3EbdnmFggCZIQwbYPgTTSGNRKkbpw9iOdzGxGxhZIOCZGFydHllMC42LjGCamFydHlfdG9raW9lMC4yLjGCZXRva2lvZjEuNTQuMQ
 [__link0]: https://crates.io/crates/arty/0.6.1
 [__link1]: https://docs.rs/arty_tokio/0.2.1/arty_tokio/?search=TokioCompatExt::tokio_compat
 [__link2]: https://docs.rs/arty_tokio/0.2.1/arty_tokio/?search=TokioCompatExt::tokio_compat_with
 [__link3]: https://docs.rs/arty_tokio/0.2.1/arty_tokio/?search=TokioCompatExt::tokio_compat_with
 [__link4]: https://docs.rs/arty_tokio/0.2.1/arty_tokio/?search=TokioHandle::default
 [__link5]: https://docs.rs/tokio/1.54.1/tokio/?search=runtime::Runtime::block_on
 [__link6]: https://docs.rs/arty/0.6.1/arty/?search=task::Context
