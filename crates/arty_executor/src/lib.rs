// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// TODO(doc-coverage): remove once `missing_docs` is promoted to [workspace.lints.rust].
#![deny(missing_docs)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Async task executor for the Arty Runtime.
//!
//! `arty_executor` is an internal implementation detail of the [`arty`](https://docs.rs/arty)
//! crate. It is responsible for every stage of task execution, including registration, polling,
//! wake processing, cancellation, completion, telemetry, and panic handling. Applications should
//! use `arty` instead of depending on this crate directly.
//!
//! The purpose of the executor is to ensure that async tasks registered with the executor make
//! progress, quickly and efficiently reacting to `await` statements that complete.
//!
//! The executor is the internal building block through which `arty` implements task execution.
//! Application code interacts with `arty`'s own task and join APIs and has no direct interaction
//! with this crate.
//!
//! # Design tenets
//!
//! The executor is single-threaded. If you want to execute tasks on multiple threads, you need to
//! run one executor on each thread. At most one executor may be alive on a thread; a replacement
//! may be created after the previous executor is dropped. Runtime integrations provide a
//! [`TaskCompletion`] target when they register a task.
//!
//! Tasks cannot be taken out of the executor - the only way for them to end is to either end
//! naturally (with the future returning `Poll::Ready`) or for the executor to be shut down. Not
//! only is there no "remove" function but similarly, there is no "cancel" function - once a task
//! has started executing, the only thing that can terminate it is the task itself, by completing.
//!
//! In a steady state, the executor is allocation-free, as all memory used by the executor is
//! reused for new tasks when old ones complete.
//!
//! Cloned task wakers may outlive their tasks. After completion or cancellation, an escaped waker
//! remains callable but becomes an inert no-op and cannot delay executor shutdown.
//!
//! Task panics are contained. [`ExecutorBuilder::panic_policy`] selects
//! whether they start executor shutdown.

mod builder;
mod cycle_outcome;
mod executor;
mod executor_context;
mod executor_shared;
mod panic;
mod panic_policy;
mod shutdown;
mod task;
mod task_handle;
mod task_list;
mod task_options;
mod task_set;
mod telemetry;
mod wake;

pub use builder::*;
pub use cycle_outcome::*;
pub use executor::*;
pub(crate) use executor_shared::*;
pub use panic::*;
pub use panic_policy::*;
pub use shutdown::*;
pub use task::TaskCompletion;
pub(crate) use task::*;
pub(crate) use task_handle::*;
pub(crate) use task_list::*;
pub use task_options::*;
pub use task_set::*;
pub(crate) use wake::*;
