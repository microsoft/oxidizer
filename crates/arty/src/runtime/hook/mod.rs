// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime events with worker-local callback data.
//!
//! Arty creates each event and passes it as the first callback argument. The
//! second argument is callback data supplied during registration. Arty clones
//! that data for each worker and relocates it before the callback can run.
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
//! use arty::runtime::hook::{OnPanic, OnStart};
//!
//! let runtime = Runtime::builder()
//!     .on::<OnStart>(|event| {
//!         let _worker = event.thread();
//!     })
//!     .on::<OnPanic>(|event| event.ignore())
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

pub use on_panic::OnPanic;
pub use on_start::OnStart;
pub(crate) use registry::{BlockingPanicHook, HookRegistration, Hooks, PanicAction};

/// Marks an event that can be registered as a runtime hook.
///
/// This trait is sealed. Only Arty can implement it.
pub trait Hook: registry::private::Sealed + 'static {}

impl Hook for OnStart {}
impl Hook for OnPanic {}
