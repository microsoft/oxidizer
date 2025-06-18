// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This is the Oxidizer Runtime that the Oxidizer SDK relies upon for much of its functionality.
//!
//! Its main responsibility is task scheduling.

// Public API surface.
mod builder;
mod error;
mod join;
mod runtime;
mod tasks;

pub use builder::*;
pub use error::*;
pub use join::*;
pub use runtime::*;
pub use tasks::*;

// Internal to the crate but re-exported at crate root for reduced hassle.
mod constants;
mod dispatch;
mod executor;
mod io;
mod wakers;
mod workers;
mod yielding;

pub(crate) use constants::ERR_POISONED_LOCK;
#[allow(clippy::wildcard_imports, reason = "TODO: Remove this wildcard import")]
pub(crate) use dispatch::*;
#[allow(clippy::wildcard_imports, reason = "TODO: Remove this wildcard import")]
pub(crate) use executor::*;
pub(crate) use io::{IoDispatch, WakerFacade, WakerWaiterFacade};
#[allow(clippy::wildcard_imports, reason = "TODO: Remove this wildcard import")]
pub(crate) use wakers::*;
#[allow(clippy::wildcard_imports, reason = "TODO: Remove this wildcard import")]
pub(crate) use workers::*;
pub(crate) use yielding::YieldFuture;

// Not re-exported internals because the module name is an important identifying factor.
mod non_blocking_thread;
mod once_event;

// These are just special.
mod macros;

#[cfg(feature = "macros")]
pub use macros::{main, test};