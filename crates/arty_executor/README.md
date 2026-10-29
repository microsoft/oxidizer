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

Async task executor for the Arty Runtime.

`arty_executor` is an internal implementation detail of the [`arty`][__link0]
crate. It is responsible for every stage of task execution, including registration, polling,
wake processing, cancellation, completion, telemetry, and panic handling. Applications should
use `arty` instead of depending on this crate directly.

The purpose of the executor is to ensure that async tasks registered with the executor make
progress, quickly and efficiently reacting to `await` statements that complete.

The executor is the internal building block through which `arty` implements task execution.
Application code interacts with `arty`’s own task and join APIs and has no direct interaction
with this crate.

## Design tenets

The executor is single-threaded. If you want to execute tasks on multiple threads, you need to
run one executor on each thread. At most one executor may be alive on a thread; a replacement
may be created after the previous executor is dropped. Runtime integrations provide a
[`TaskCompletion`][__link1] target when they register a task.

Tasks cannot be taken out of the executor - the only way for them to end is to either end
naturally (with the future returning `Poll::Ready`) or for the executor to be shut down. Not
only is there no “remove” function but similarly, there is no “cancel” function - once a task
has started executing, the only thing that can terminate it is the task itself, by completing.

In a steady state, the executor is allocation-free, as all memory used by the executor is
reused for new tasks when old ones complete.

Cloned task wakers may outlive their tasks. After completion or cancellation, an escaped waker
remains callable but becomes an inert no-op and cannot delay executor shutdown.

Task panics are contained. [`ExecutorBuilder::panic_policy`][__link2] selects
whether they start executor shutdown.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_executor">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbIYmcOKYv51sbz18Vri9YSwUbWHZTVjH_zxUbmqJfUAMYjZNhZIKCb0V4ZWN1dG9yQnVpbGRlcvaCbWFydHlfZXhlY3V0b3JlMC41LjA
 [__link0]: https://docs.rs/arty
 [__link1]: https://docs.rs/arty_executor/0.5.0/arty_executor/?search=TaskCompletion
 [__link2]: https://docs.rs/ExecutorBuilder/latest/ExecutorBuilder/?search=panic_policy
