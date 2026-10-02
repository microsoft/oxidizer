<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Zygote Rt Logo" width="96">

# Zygote Rt

[![crate.io](https://img.shields.io/crates/v/zygote_rt.svg)](https://crates.io/crates/zygote_rt)
[![docs.rs](https://docs.rs/zygote_rt/badge.svg)](https://docs.rs/zygote_rt)
[![MSRV](https://img.shields.io/crates/msrv/zygote_rt)](https://crates.io/crates/zygote_rt)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Integrates an executable with `zygote_control` for repeated launches.

Add this crate to the executable that a `zygote_control::Zygote` launches.
The executable remains directly runnable. On Linux, the controller can
reuse an initialized template; on other platforms, the integration invokes
the application normally for each native process.

## Choose an integration mode

|Mode|Use it when|
|----|-----------|
|**Transparent**|The existing `main` should run unchanged and startup before `main` is inexpensive.|
|**Prepared**|Expensive immutable state can safely be constructed once and shared with every launched child.|

Transparent mode keeps an existing `main` unchanged. Prepared mode lets the
target construct immutable state once per template and supplies that state
to each launched child.

## Target setup

Add the runtime as both a normal dependency and a build dependency:

```toml
[dependencies]
zygote_rt = "0.1"

[build-dependencies]
zygote_rt = { version = "0.1", features = ["build"] }
```

Add a `build.rs` that configures the final executable:

```text
fn main() {
    zygote_rt::build::configure();
}
```

On Linux, the final linker must support GNU-style `--wrap`. The crate uses
checked-in Rust sources and does not require a C compiler or `protoc`.

## Transparent mode

A transparent target keeps an ordinary `main`. Direct execution and
controller launches invoke that same function with the selected arguments,
environment, working directory, and standard streams:

```rust
zygote_rt::link!();

fn main() {
    println!("{:?}", std::env::args_os().collect::<Vec<_>>());
}
```

Place [`link!`][__link0] once at module scope. No other application changes are
required.

## Prepared mode

A prepared target constructs immutable state once per Linux template
worker. Direct execution still prepares once and runs the application
normally:

```rust
use zygote_rt::{Launch, Prepared, ZygoteSafe};

struct State {
    greeting: String,
}

// This is sound because State contains immutable owned data and no threads,
// locks, process-specific handles, or pointers into external storage.
unsafe impl ZygoteSafe for State {}

fn prepare() -> Result<Prepared<State>, &'static str> {
    Ok(Prepared::new(State {
        greeting: "hello".to_owned(),
    }))
}

fn application(state: &'static State, launch: Launch<'_>) -> i32 {
    println!(
        "{} {:?}",
        state.greeting,
        launch.args_os().collect::<Vec<_>>()
    );
    0
}

zygote_rt::prepared_main!(prepare, application);
```

The application callback receives the same prepared state for every launch
from a template and a [`Launch`][__link1] containing that launch’s arguments.
[`Launch::args_os`][__link2] borrows those arguments for the callback; use
[`Launch::into_args_os`][__link3] when they must be retained.

## Prepared-state safety

Implementing [`ZygoteSafe`][__link4] is an explicit safety assertion. Prepared state
must not contain live threads, held locks, writable process-shared state,
child-specific secrets, or kernel resources whose duplication would couple
launches. Prefer immutable owned data such as parsed configuration,
lookup tables, and read-only model data.

Preparation should finish before starting thread pools, asynchronous
runtimes, network clients, or telemetry exporters. Initialize those
child-specific resources inside the application callback instead.
Preparation failure exits without accepting launches.
On Linux, `Prepared::protected` can place the root value in a dedicated
mapping that becomes read-only before launches begin. This is not recursive:
heap allocations referenced by the root retain their original protection.

## Launch behavior

In transparent mode, use [`std::env::args_os`][__link5] as usual. In prepared mode,
use [`Launch::args_os`][__link6]. In both modes, `zygote_control` applies the
requested environment, working directory, standard streams, process
settings, and sandbox before application code runs.

Each configured Linux worker prepares independently. Prepared data is
inherited through copy-on-write memory, so keeping it immutable preserves
sharing; writing to inherited pages increases each child’s private memory.
The controller reports preparation, integration, and specialization
failures rather than silently launching without acceleration.
When the controller enables its prefork pool, dormant single-use workers
inherit this same prepared snapshot and receive launch-specific state only
after assignment.

## Integration checklist

1. Add `zygote_rt` as both a normal and build dependency.
1. Call `zygote_rt::build::configure()` from the target’s `build.rs`.
1. Choose exactly one entry mode: [`link!`][__link7] or [`prepared_main!`][__link8].
1. Keep template initialization single-threaded.
1. Launch the resulting executable through `zygote_control::Zygote`.

Complete runnable targets, their manifest, and their build script are
maintained in the
[integration fixtures][__link9].


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/zygote_rt">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb6hcIJnrQMwsbdEKapOUn2-MbZCmY0Aov-_EbHNoyxiO1tYxhZIGCaXp5Z290ZV9ydGUwLjEuMA
 [__link0]: https://docs.rs/zygote_rt/0.1.0/zygote_rt/macro.link.html
 [__link1]: https://docs.rs/zygote_rt/0.1.0/zygote_rt/?search=Launch
 [__link2]: https://docs.rs/zygote_rt/0.1.0/zygote_rt/?search=Launch::args_os
 [__link3]: https://docs.rs/zygote_rt/0.1.0/zygote_rt/?search=Launch::into_args_os
 [__link4]: https://docs.rs/zygote_rt/0.1.0/zygote_rt/?search=ZygoteSafe
 [__link5]: https://doc.rust-lang.org/stable/std/?search=env::args_os
 [__link6]: https://docs.rs/zygote_rt/0.1.0/zygote_rt/?search=Launch::args_os
 [__link7]: https://docs.rs/zygote_rt/0.1.0/zygote_rt/macro.link.html
 [__link8]: https://docs.rs/zygote_rt/0.1.0/zygote_rt/macro.prepared_main.html
 [__link9]: https://github.com/microsoft/oxidizer/tree/main/crates/zygote_control/test-fixtures/targets
