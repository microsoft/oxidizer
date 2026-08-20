<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Executor Logo" width="96">

# Arty Executor

[![crate.io](https://img.shields.io/crates/v/arty_executor.svg)](https://crates.io/crates/arty_executor)
[![docs.rs](https://docs.rs/arty_executor/badge.svg)](https://docs.rs/arty_executor)
[![MSRV](https://img.shields.io/crates/msrv/arty_executor)](https://crates.io/crates/arty_executor)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Async task executor for the Arty Runtime.

The purpose of the executor is to ensure that async tasks registered with the executor make
progress, quickly and efficiently reacting to `await` statements that complete.

The executor is a building block for an application runtime that provides foundational
capabilities like task execution, multithreaded task management, I/O, timers, and more.
Application logic may encounter [`JoinHandle`][__link0]s but otherwise has no direct interaction
with the executor.

## Design tenets

The executor is single-threaded. If you want to execute tasks on multiple threads, you need to
run multiple executors on different threads. If you wish to observe task results across
threads, you need to create a mechanism to ship the results across thread boundaries.

Tasks cannot be taken out of the executor - the only way for them to end is to either end
naturally (with the future returning `Poll::Ready`) or for the executor to be shut down. Not
only is there no “remove” function but similarly, there is no “cancel” function - once a task
has started executing, the only thing that can terminate it is the task itself, by completing.

In a steady state, the executor is allocation-free, as all memory used by the executor is
reused for new tasks when old ones complete.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_executor">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbVwz61IbZe5QbF4vbEa1LIsAbVDxflkNvtrIbD-TpXycN1glhZIGCakpvaW5IYW5kbGX2
 [__link0]: https://crates.io/crates/JoinHandle
