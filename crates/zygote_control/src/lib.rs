// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(missing_docs)]

//! Repeatedly launches one executable through a `std::process`-style API.
//!
//! Use `zygote_control` when an application starts many short-lived instances
//! of the same program. A [`Zygote`] fixes the executable and owns the launch
//! backend. Each [`Command`] configures one child process, and a cloneable
//! [`Launcher`] lets multiple threads submit launches.
//!
//! Linux can reuse target initialization to reduce startup latency. Windows
//! and macOS use native process creation behind the same core API, making it
//! possible to keep portable launch code while opting into Linux acceleration.
//!
//! # Getting started
//!
//! Add this crate to the controller application's manifest:
//!
//! ```toml
//! [dependencies]
//! zygote_control = "0.1"
//! ```
//!
//! For accelerated Linux launches, the target executable must also integrate
//! `zygote_rt`. Integration does not prevent users or other tools from running
//! the executable directly. Native backends do not require this integration.
//!
//! ```no_run
//! use std::io;
//!
//! use zygote_control::Zygote;
//!
//! fn main() -> io::Result<()> {
//!     let zygote = Zygote::builder("/path/to/integrated-target").spawn()?;
//!     let output = zygote
//!         .command()
//!         .args(["convert", "--format=json"])
//!         .env("REQUEST_ID", "42")
//!         .output()?;
//!
//!     assert!(output.status.success());
//!     println!("{}", String::from_utf8_lossy(&output.stdout));
//!     Ok(())
//! }
//! ```
//!
//! # Platform behavior
//!
//! | Platform | Launch behavior | Platform-specific configuration |
//! |---|---|---|
//! | Linux | Reuses one or more initialized target templates. | `linux::CommandExt` adds seccomp, Landlock, cgroup, namespace, and privilege controls. |
//! | Windows | Creates a native process for each launch. | `windows::CommandExt` adds Job Objects, restricted tokens, `AppContainer` identities, mitigations, and handle allowlists. |
//! | Other Unix targets | Creates a native process for each launch. | `unix::CommandExt` adds credentials, groups, sessions, process groups, umask, and resource limits. |
//!
//! Linux startup fails if the target is not correctly integrated; it never
//! silently falls back to a slower launch path. Platform-specific modules are
//! available only on their corresponding targets.
//!
//! Accelerated Linux templates start with only authenticated bootstrap
//! variables; they do not inherit the controller environment. Per-launch
//! environment values are installed in each forked child after the template
//! executable and its shared libraries have loaded. Loader-time variables such
//! as `LD_LIBRARY_PATH` and `LD_PRELOAD` therefore cannot be supplied through
//! [`Command`]. Linux targets must resolve their initial dependencies through
//! standard loader paths or embedded paths such as `RPATH`/`RUNPATH`.
//!
//! # Configure a launch
//!
//! [`Command`] supports arguments, environment changes, a working directory,
//! standard streams, portable sandbox requirements, and platform extensions.
//! Its builder methods intentionally resemble [`std::process::Command`].
//! [`Command::spawn`] returns a [`Child`], while [`Command::status`] waits for
//! an exit status and [`Command::output`] captures output.
//!
//! Captured output is bounded by default to 8 MiB per stream and 16 MiB total.
//! Use [`OutputLimits`] and [`Command::output_limits`] when a workload needs
//! different limits. Exceeding a limit returns an error and triggers bounded
//! child cleanup instead of allowing the controller to grow memory without
//! limit.
//!
//! # Concurrency and lifecycle
//!
//! [`Launcher`] is cloneable and may be shared across threads. One Linux
//! template starts requests serially, although the launched processes run
//! concurrently. Configure multiple workers when launch bursts need parallel
//! startup. Each worker has its own initialized template and therefore its own
//! copy-on-write memory cost. A worker accepts at most 64 in-flight launch
//! requests; additional launches return [`std::io::ErrorKind::WouldBlock`].
//! Linux callers can configure [`PreforkPoolConfig`] to keep dormant,
//! single-use workers ready for burst launches, and [`WorkerRecoveryPolicy`]
//! to replace failed templates without replaying ambiguous requests.
//! [`Launcher::health`] and [`Zygote::health`] return lock-free operational
//! snapshots, while [`Zygote::trim_prefork_pool`] adjusts dormant capacity
//! explicitly under memory pressure.
//!
//! ```no_run
//! use std::io;
//!
//! use zygote_control::Zygote;
//!
//! fn main() -> io::Result<()> {
//!     let mut builder = Zygote::builder("/path/to/integrated-target");
//!     builder.workers(2)?;
//!     let zygote = builder.spawn()?;
//!     let launcher = zygote.launcher();
//!     let workers = ["first", "second"].map(|argument| {
//!         let launcher = launcher.clone();
//!         std::thread::spawn(move || launcher.command().arg(argument).status())
//!     });
//!     for worker in workers {
//!         assert!(
//!             worker
//!                 .join()
//!                 .expect("launch thread must not panic")?
//!                 .success()
//!         );
//!     }
//!     Ok(())
//! }
//! ```
//!
//! Dropping or shutting down the [`Zygote`] stops future launches but does not
//! terminate children that have already started. Children remain independently
//! queryable and terminable. On accelerated Linux, shutdown may make an
//! uncached exit status unavailable because the controller is not the child's
//! parent; [`Child::wait`] then returns an error.
//!
//! # Sandboxing
//!
//! [`SandboxPolicy`] expresses guarantees that portable code can require.
//! Platform modules expose stronger native policies. Requested guarantees are
//! fail-closed: invalid or unsupported policies return an error before
//! application code begins rather than silently weakening the launch.
//!
//! [`SandboxPolicy::probe`] checks whether the target family implements a
//! portable policy; it does not inspect runtime host capabilities. Platform
//! `Support::probe` APIs provide capability details where available. Every
//! launch still validates and applies the complete policy.
//!
//! Linux cgroup membership and Windows Job Object limits are intentionally
//! distinct APIs: the former joins a hierarchy configured by the caller,
//! while the latter configures per-launch commit, working-set, CPU, process
//! count, and lifetime limits. Neither is presented as an exact physical-RAM
//! quota.
//!
//! # Differences from `std::process`
//!
//! The executable cannot change after [`Zygote::builder`]. Arguments, the
//! application environment, working directory, and standard streams remain
//! per-launch. On accelerated Linux launches, the application receives the
//! requested arguments, but tools that inspect the kernel's original
//! command-line memory may still display the template invocation.
//!
//! See the [design document][design] for deployment requirements, detailed
//! process semantics, and the prepared-state safety model.
//!
//! [design]: https://github.com/microsoft/oxidizer/blob/main/crates/zygote_control/docs/DESIGN.md

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/zygote_control/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/zygote_control/favicon.ico")]

mod child;
mod command;
#[cfg(target_os = "linux")]
pub mod linux;
mod sandbox;
mod sealed {
    pub trait Sealed {}

    impl Sealed for crate::Command {}
}
mod stdio;
#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;
mod zygote;

#[doc(inline)]
pub use child::{Child, ChildStderr, ChildStdin, ChildStdout, OutputLimits};
#[doc(inline)]
pub use command::Command;
#[doc(inline)]
pub use sandbox::{PrivilegeIntent, SandboxPolicy};
#[doc(inline)]
pub use stdio::Stdio;
#[doc(inline)]
pub use zygote::{Launcher, LauncherHealth, PreforkPoolConfig, WorkerRecoveryPolicy, Zygote, ZygoteBuilder};
