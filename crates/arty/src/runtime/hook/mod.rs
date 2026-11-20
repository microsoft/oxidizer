// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callbacks for runtime events.
//!
//! This module provides these hooks:
//!
//! - [`TaskSpawn`] runs when Arty submits an async task.
//! - [`WorkerSpawn`][^worker-spawn-retention] runs once on each async worker after
//!   the worker initializes.
//! - [`TaskPanic`] runs when Arty contains a panic from an async task factory or
//!   task poll. The callback can stop the runtime or keep it running.
//!
//! [^worker-spawn-retention]: [`WorkerSpawn`] has retained registration semantics.
//! Arty runs the callback on current workers and on workers that start after
//! registration.
//!
//! Arty clones the data for each worker and relocates it before a callback can
//! run there. Task-spawn hooks also keep a worker-neutral instance for submissions
//! outside runtime workers. Neutral callbacks can run concurrently. Function
//! pointers prevent hidden closure captures from bypassing [`ThreadAware`]
//! relocation.
//!
//! Register hooks before startup with [`RuntimeBuilder::on`]. Add hooks after
//! startup with [`RuntimeControl::on`]. Before startup completes, each initial
//! worker runs its [`WorkerSpawn`] callbacks from the builder.
//!
//! Arty does not catch a panic from a hook callback. You must make sure that
//! every hook callback completes without a panic.
//!
//! ```
//! use arty::runtime::Runtime;
//! use arty::runtime::hook::TaskPanic;
//!
//! let runtime = Runtime::builder().on(TaskPanic::ignore()).build()?;
//! runtime.stop()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! [`RuntimeBuilder::on`]: crate::runtime::RuntimeBuilder::on
//! [`RuntimeControl::on`]: crate::runtime::RuntimeControl::on
//! [`ThreadAware`]: crate::core::ThreadAware

mod hook_fn;
mod local_hook;
mod registry;
mod retained_hooks;
mod task_panic;
mod task_spawn;
mod worker_spawn;

pub(crate) use hook_fn::ErasedHookFn;
pub use hook_fn::HookFn;
pub(crate) use local_hook::{LocalHook, LocalHookGuard, LocalHookStorage};
pub(crate) use registry::{HookRegistration, RegisterHook, registration};
pub(crate) use retained_hooks::RetainedHooks;
pub use task_panic::TaskPanic;
pub(crate) use task_spawn::TaskObservers;
pub use task_spawn::{TaskObserver, TaskSpawn};
pub use worker_spawn::WorkerSpawn;

/// Marks an event that can be registered as a runtime hook.
///
/// This trait is sealed. Only Arty can implement it.
#[expect(private_bounds, reason = "Hook uses the sealed trait pattern")]
pub trait Hook: private::Sealed + 'static {}

mod private {
    pub(crate) trait Sealed: Sized {}
}
