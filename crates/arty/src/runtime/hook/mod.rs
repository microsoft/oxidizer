// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime hooks with worker-local callback data.
//!
//! Arty passes each callback a mutable hook and shared callback data. Arty
//! clones the data for each worker and relocates it before the callback can run.
//! Function pointers prevent hidden closure captures from bypassing
//! [`ThreadAware`] relocation.
//!
//! Register hooks before startup with [`RuntimeBuilder::on`]. Add hooks after
//! startup with [`RuntimeControl::on`]. Each worker stores one collection for
//! each event. Arty invokes callbacks in registration order.
//! Hook callbacks must not panic.
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
mod panic;
mod registry;

pub use hook_fn::HookFn;
pub use panic::Panic;
pub(crate) use registry::{HookRegistration, Hooks, PanicAction, registration};

/// Marks an event that can be registered as a runtime hook.
///
/// This trait is sealed. Only Arty can implement it.
#[allow(private_bounds)]
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
