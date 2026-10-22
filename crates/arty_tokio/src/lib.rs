// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![warn(missing_docs)]

//! Tokio compatibility for tasks running on [`arty`].
//!
//! Call [`init`] once at the start of an Arty task. It enters one Tokio runtime
//! context on every fixed Arty worker. It returns after all workers are ready.
//!
//! # Default runtime
//!
//! [`TokioHandle::default`] asks [`init`] to build one shared, multi-thread
//! Tokio runtime:
//!
//! ```
//! use arty::task::Builtins;
//! use arty_tokio::TokioHandle;
//!
//! # #[arty::main]
//! # async fn main(cx: Builtins) -> Result<(), ohno::AppError> {
//! arty_tokio::init(&cx, TokioHandle::default()).await?;
//! let value = tokio::spawn(async { 42 })
//!     .await
//!     .expect("Tokio task must complete");
//! assert_eq!(value, 42);
//! Ok(())
//! # }
//! ```
//!
//! The crate owns this runtime until Arty stops. It uses
//! [`tokio::runtime::Builder::new_multi_thread`] and Tokio's default worker
//! count. It also calls [`tokio::runtime::Builder::enable_all`].
//!
//! # Initialization failure
//!
//! Initialization prepares every worker before it commits. If a worker fails,
//! [`init`] removes each Tokio context that it installed. Canceling [`init`]
//! also starts this cleanup. A later call can retry on the same Arty runtime
//! after cleanup completes.
//!
//! # External runtime
//!
//! Pass a multi-thread Tokio [`Handle`] to use a Tokio
//! runtime that your application owns:
//!
//! ```
//! use arty::task::Builtins;
//! use tokio::runtime::Builder;
//!
//! # #[arty::main]
//! # async fn main(cx: Builtins) -> Result<(), ohno::AppError> {
//! let tokio = Builder::new_multi_thread().enable_all().build()?;
//! arty_tokio::init(&cx, tokio.handle().clone()).await?;
//! let value = tokio::spawn(async { 42 })
//!     .await
//!     .expect("Tokio task must complete");
//! assert_eq!(value, 42);
//! # Ok(())
//! # }
//! ```
//!
//! The crate stores only the handle. It does not own the supplied runtime. Keep
//! that runtime alive while Arty uses Tokio. [`init`] rejects current-thread
//! Tokio runtimes.
//!
//! # Compatibility
//!
//! After successful initialization, normal Arty task execution on every worker
//! supports:
//!
//! - [`tokio::runtime::Handle::current`] and
//!   [`tokio::runtime::Handle::try_current`];
//! - [`tokio::spawn`] and [`tokio::task::spawn_blocking`] submission;
//! - Tokio timers, networking, and I/O resources with enabled drivers;
//! - directly awaiting Tokio-based futures from Arty tasks.
//!
//! Tokio tasks run on Tokio workers. They can move between Tokio workers. They
//! do not keep Arty thread-aware state or receive [`arty::task::Builtins`].
//! They have no affinity to the Arty worker that submitted them. Tokio
//! [`JoinHandle`](tokio::task::JoinHandle) and
//! [`JoinError`](tokio::task::JoinError) behavior does not change.
//!
//! The crate does not support these APIs or behaviors:
//!
//! - current-thread Tokio runtimes and implicit `LocalSet` / `spawn_local`
//!   integration;
//! - [`tokio::task::block_in_place`] from an Arty worker;
//! - `Runtime::block_on` or `Handle::block_on` from an Arty worker;
//! - Tokio cooperative budgets or Tokio task-local executor context when Arty
//!   directly polls a Tokio future. Run the library root inside
//!   [`tokio::spawn`];
//! - Tokio context during final Arty timer or worker teardown callbacks.
//!
//! # Shutdown
//!
//! Installer tasks remain parked until Arty stops. Arty drops each installer on
//! its worker. The installer leaves the Tokio context before the crate drops an
//! internally owned runtime. Tokio waits for blocking work to finish. Long
//! [`tokio::task::spawn_blocking`] work can delay Arty shutdown.

use std::fmt;

use tokio::runtime::Handle;

mod error;
mod initialization;

pub use error::InitError;

/// Selects the Tokio runtime for [`init`].
///
/// Use [`Default`] for a shared runtime that this crate builds and owns. You can
/// also convert an owned multi-thread Tokio [`Handle`].
///
/// Borrowed handles are not accepted:
///
/// ```compile_fail
/// use arty_tokio::TokioHandle;
///
/// let runtime = tokio::runtime::Builder::new_multi_thread().build().unwrap();
/// let _: TokioHandle = runtime.handle().into();
/// ```
///
/// Owned runtimes are not accepted:
///
/// ```compile_fail
/// use arty_tokio::TokioHandle;
///
/// let runtime = tokio::runtime::Builder::new_multi_thread().build().unwrap();
/// let _: TokioHandle = runtime.into();
/// ```
pub struct TokioHandle {
    kind: TokioHandleKind,
}

enum TokioHandleKind {
    Default,
    External(Handle),
}

impl Default for TokioHandle {
    fn default() -> Self {
        Self {
            kind: TokioHandleKind::Default,
        }
    }
}

impl From<Handle> for TokioHandle {
    fn from(handle: Handle) -> Self {
        Self {
            kind: TokioHandleKind::External(handle),
        }
    }
}

impl fmt::Debug for TokioHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokioHandle").finish_non_exhaustive()
    }
}

/// Enters one Tokio runtime context on every worker of the current Arty runtime.
///
/// `tokio` may be [`TokioHandle::default`] or an owned multi-thread Tokio
/// [`Handle`]. The function returns after every fixed worker enters the same
/// Tokio runtime context. The contexts remain until Arty stops.
///
/// # Errors
///
/// Returns [`InitError`] if the crate cannot enter the context on every worker.
/// The function removes every context from this attempt before it returns an
/// error. Canceling the future starts the same cleanup without waiting for it.
pub async fn init(cx: &arty::task::Builtins, tokio: impl Into<TokioHandle>) -> Result<(), InitError> {
    initialization::initialize_all_workers(cx, tokio.into()).await
}
