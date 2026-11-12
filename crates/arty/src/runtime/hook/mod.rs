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
//! startup with [`RuntimeControl::on`]. Each worker stores one typed collection
//! for each event. Arty invokes callbacks in registration order.
//! Hook callbacks must not panic.
//!
//! ```
//! use arty::runtime::Runtime;
//! use arty::runtime::hook::{HookFn, OnPanic, OnStart};
//!
//! let runtime = Runtime::builder()
//!     .on(HookFn::<OnStart, _>::new((), |event, ()| {
//!         let _worker = event.thread();
//!     }))
//!     .on(OnPanic::IGNORE)
//!     .build()?;
//! runtime.stop()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! [`RuntimeBuilder::on`]: crate::runtime::RuntimeBuilder::on
//! [`RuntimeControl::on`]: crate::runtime::RuntimeControl::on
//! [`ThreadAware`]: crate::core::ThreadAware

mod on_panic;
mod on_start;
mod registry;
mod typed;

pub use on_panic::OnPanic;
pub use on_start::OnStart;
pub(crate) use registry::{HookRegistration, Hooks, PanicAction};
pub use typed::HookFn;

/// Marks an event that can be registered as a runtime hook.
///
/// This trait is sealed. Only Arty can implement it.
pub trait Hook: registry::private::Sealed + 'static {}
