// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callbacks for runtime events.
//!
//! This module provides these hooks:
//!
//! - [`WorkerInit`] runs once on each async worker after the worker initializes.
//!   It also runs on workers that start after registration.
//! - [`Panic`] runs when Arty contains a panic from an async task factory or
//!   task poll. The callback can stop the runtime or keep it running.
//!
//! Arty passes each callback a mutable hook and shared callback data. Arty
//! clones the data for each worker and relocates it before the callback can run.
//! Function pointers prevent hidden closure captures from bypassing
//! [`ThreadAware`] relocation.
//!
//! Register hooks before startup with [`RuntimeBuilder::on`]. Add hooks after
//! startup with [`RuntimeControl::on`]. Before startup completes, each initial
//! worker runs its [`WorkerInit`] callbacks from the builder.
//!
//! Arty does not catch a panic from a hook callback. You must make sure that
//! every hook callback completes without a panic.
//!
//! ```
//! use arty::runtime::Runtime;
//! use arty::runtime::hook::Panic;
//!
//! let runtime = Runtime::builder().on(Panic::IGNORE).build()?;
//! runtime.stop()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! [`RuntimeBuilder::on`]: crate::runtime::RuntimeBuilder::on
//! [`RuntimeControl::on`]: crate::runtime::RuntimeControl::on
//! [`ThreadAware`]: crate::core::ThreadAware

mod hook_fn;
mod local_hook;
mod panic;
mod registry;
mod worker_init;

pub(crate) use hook_fn::ErasedHookFn;
pub use hook_fn::HookFn;
pub(crate) use local_hook::{LocalHook, LocalHookGuard};
pub use panic::Panic;
pub(crate) use registry::{HookRegistration, registration};
pub use worker_init::WorkerInit;

/// Marks an event that can be registered as a runtime hook.
///
/// This trait is sealed. Only Arty can implement it.
#[expect(private_bounds, reason = "Hook uses the sealed trait pattern")]
pub trait Hook: private::Sealed + 'static {}

mod private {
    use thread_aware::ThreadAware;

    use super::{HookFn, HookRegistration};

    pub(crate) trait Sealed: Sized {
        type Erased<D>: From<HookFn<Self, D>> + Into<HookRegistration>
        where
            Self: super::Hook,
            D: ThreadAware + Clone + 'static;
    }
}
