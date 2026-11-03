<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Executor Logo" width="96">

# Arty Executor

[![crate.io](https://img.shields.io/crates/v/arty_executor.svg)](https://crates.io/crates/arty_executor)
[![docs.rs](https://docs.rs/arty_executor/badge.svg)](https://docs.rs/arty_executor)
[![MSRV](https://img.shields.io/crates/msrv/arty_executor)](https://crates.io/crates/arty_executor)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Single-threaded async task executor for the Arty Runtime.

`arty_executor` is an internal part of the [`arty`][__link0] crate.
It registers, polls, wakes, cancels, and completes tasks. It also reports telemetry
and contains task panics. Applications use `arty` instead of this crate.

`arty` uses this crate to run async tasks. Application code uses the task and join
APIs from `arty`.

## Design

The executor runs on one thread. Run one executor on each thread that needs to execute
tasks. Only one executor can be alive on a thread. You can create a replacement after
you drop the previous executor. A runtime integration supplies a [`TaskCompletion`][__link1]
target when it registers a task.

A task ends when its future returns `Poll::Ready`, when it panics, or when
executor shutdown cancels it. The executor does not provide methods to
remove or cancel one task.

During steady operation, the executor reuses task storage and does not allocate.

A cloned task waker can outlive its task. After task completion or cancellation,
the waker does nothing and cannot delay executor shutdown.

The executor contains task panics. Use
[`ExecutorBuilder::panic_policy`][__link2] to select
whether a task panic starts executor shutdown.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_executor">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb4FdV-PCtKaQbxkXaKKYzz34bu-_H5a7dp90bKTfNsPxh4HthZIGCbWFydHlfZXhlY3V0b3JlMC42LjA
 [__link0]: https://docs.rs/arty
 [__link1]: https://docs.rs/arty_executor/0.6.0/arty_executor/?search=TaskCompletion
 [__link2]: https://docs.rs/arty_executor/0.6.0/arty_executor/?search=ExecutorBuilder::panic_policy
