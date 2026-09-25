<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Zygote Control Logo" width="96">

# Zygote Control

[![crate.io](https://img.shields.io/crates/v/zygote_control.svg)](https://crates.io/crates/zygote_control)
[![docs.rs](https://docs.rs/zygote_control/badge.svg)](https://docs.rs/zygote_control)
[![MSRV](https://img.shields.io/crates/msrv/zygote_control)](https://crates.io/crates/zygote_control)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Repeatedly launches one executable through a `std::process`-style API.

Use `zygote_control` when an application starts many short-lived instances
of the same program. A [`Zygote`][__link0] fixes the executable and owns the launch
backend. Each [`Command`][__link1] configures one child process, and a cloneable
[`Launcher`][__link2] lets multiple threads submit launches.

Linux can reuse target initialization to reduce startup latency. Windows
and macOS use native process creation behind the same core API, making it
possible to keep portable launch code while opting into Linux acceleration.

## Getting started

Add this crate to the controller application’s manifest:

```toml
[dependencies]
zygote_control = "0.1"
```

For accelerated Linux launches, the target executable must also integrate
`zygote_rt`. Integration does not prevent users or other tools from running
the executable directly. Native backends do not require this integration.

```rust
use std::io;

use zygote_control::Zygote;

fn main() -> io::Result<()> {
    let zygote = Zygote::builder("/path/to/integrated-target").spawn()?;
    let output = zygote
        .command()
        .args(["convert", "--format=json"])
        .env("REQUEST_ID", "42")
        .output()?;

    assert!(output.status.success());
    println!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}
```

## Platform behavior

|Platform|Launch behavior|Platform-specific configuration|
|--------|---------------|-------------------------------|
|Linux|Reuses one or more initialized target templates.|`linux::CommandExt` adds seccomp, Landlock, cgroup, namespace, and privilege controls.|
|Windows|Creates a native process for each launch.|`windows::CommandExt` adds Job Objects, restricted tokens, `AppContainer` identities, mitigations, and handle allowlists.|
|Other Unix targets|Creates a native process for each launch.|`unix::CommandExt` adds credentials, groups, sessions, process groups, umask, and resource limits.|

Linux startup fails if the target is not correctly integrated; it never
silently falls back to a slower launch path. Platform-specific modules are
available only on their corresponding targets.

Accelerated Linux templates start with only authenticated bootstrap
variables; they do not inherit the controller environment. Per-launch
environment values are installed in each forked child after the template
executable and its shared libraries have loaded. Loader-time variables such
as `LD_LIBRARY_PATH` and `LD_PRELOAD` therefore cannot be supplied through
[`Command`][__link3]. Linux targets must resolve their initial dependencies through
standard loader paths or embedded paths such as `RPATH`/`RUNPATH`.

## Configure a launch

[`Command`][__link4] supports arguments, environment changes, a working directory,
standard streams, portable sandbox requirements, and platform extensions.
Its builder methods intentionally resemble [`std::process::Command`][__link5].
[`Command::spawn`][__link6] returns a [`Child`][__link7], while [`Command::status`][__link8] waits for
an exit status and [`Command::output`][__link9] captures output.

Captured output is bounded by default to 8 MiB per stream and 16 MiB total.
Use [`OutputLimits`][__link10] and [`Command::output_limits`][__link11] when a workload needs
different limits. Exceeding a limit returns an error and triggers bounded
child cleanup instead of allowing the controller to grow memory without
limit.

## Concurrency and lifecycle

[`Launcher`][__link12] is cloneable and may be shared across threads. One Linux
template starts requests serially, although the launched processes run
concurrently. Configure multiple workers when launch bursts need parallel
startup. Each worker has its own initialized template and therefore its own
copy-on-write memory cost. A worker accepts at most 64 in-flight launch
requests; additional launches return [`std::io::ErrorKind::WouldBlock`][__link13].
Linux callers can configure [`PreforkPoolConfig`][__link14] to keep dormant,
single-use workers ready for burst launches, and [`WorkerRecoveryPolicy`][__link15]
to replace failed templates without replaying ambiguous requests.
[`Launcher::health`][__link16] and [`Zygote::health`][__link17] return lock-free operational
snapshots, while [`Zygote::trim_prefork_pool`][__link18] adjusts dormant capacity
explicitly under memory pressure.

```rust
use std::io;

use zygote_control::Zygote;

fn main() -> io::Result<()> {
    let mut builder = Zygote::builder("/path/to/integrated-target");
    builder.workers(2)?;
    let zygote = builder.spawn()?;
    let launcher = zygote.launcher();
    let workers = ["first", "second"].map(|argument| {
        let launcher = launcher.clone();
        std::thread::spawn(move || launcher.command().arg(argument).status())
    });
    for worker in workers {
        assert!(
            worker
                .join()
                .expect("launch thread must not panic")?
                .success()
        );
    }
    Ok(())
}
```

Dropping or shutting down the [`Zygote`][__link19] stops future launches but does not
terminate children that have already started. Children remain independently
queryable and terminable. On accelerated Linux, shutdown may make an
uncached exit status unavailable because the controller is not the child’s
parent; [`Child::wait`][__link20] then returns an error.

## Sandboxing

[`SandboxPolicy`][__link21] expresses guarantees that portable code can require.
Platform modules expose stronger native policies. Requested guarantees are
fail-closed: invalid or unsupported policies return an error before
application code begins rather than silently weakening the launch.

[`SandboxPolicy::probe`][__link22] checks whether the target family implements a
portable policy; it does not inspect runtime host capabilities. Platform
`Support::probe` APIs provide capability details where available. Every
launch still validates and applies the complete policy.

Linux cgroup membership and Windows Job Object limits are intentionally
distinct APIs: the former joins a hierarchy configured by the caller,
while the latter configures per-launch commit, working-set, CPU, process
count, and lifetime limits. Neither is presented as an exact physical-RAM
quota.

## Differences from `std::process`

The executable cannot change after [`Zygote::builder`][__link23]. Arguments, the
application environment, working directory, and standard streams remain
per-launch. On accelerated Linux launches, the application receives the
requested arguments, but tools that inspect the kernel’s original
command-line memory may still display the template invocation.

See the [design document][__link24] for deployment requirements, detailed
process semantics, and the prepared-state safety model.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/zygote_control">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbao6qwSXeufIbqnVI9jOqtFobAF-aCU7fPhwbYrK4-udQl6FhZIGCbnp5Z290ZV9jb250cm9sZTAuMS4w
 [__link0]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Zygote
 [__link1]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Command
 [__link10]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=OutputLimits
 [__link11]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Command::output_limits
 [__link12]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Launcher
 [__link13]: https://doc.rust-lang.org/stable/std/?search=io::ErrorKind::WouldBlock
 [__link14]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=PreforkPoolConfig
 [__link15]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=WorkerRecoveryPolicy
 [__link16]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Launcher::health
 [__link17]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Zygote::health
 [__link18]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Zygote::trim_prefork_pool
 [__link19]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Zygote
 [__link2]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Launcher
 [__link20]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Child::wait
 [__link21]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=SandboxPolicy
 [__link22]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=SandboxPolicy::probe
 [__link23]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Zygote::builder
 [__link24]: https://github.com/microsoft/oxidizer/blob/main/crates/zygote_control/docs/DESIGN.md
 [__link3]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Command
 [__link4]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Command
 [__link5]: https://doc.rust-lang.org/stable/std/?search=process::Command
 [__link6]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Command::spawn
 [__link7]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Child
 [__link8]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Command::status
 [__link9]: https://docs.rs/zygote_control/0.1.0/zygote_control/?search=Command::output
