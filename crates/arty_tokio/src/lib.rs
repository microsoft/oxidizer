// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Lets [`arty`] tasks use Tokio-based libraries.
//!
//! Call [`init`] from an Arty task during application startup. It enters the
//! same Tokio runtime context on every fixed Arty worker and returns when all
//! workers are ready.
//!
//! # Default runtime
//!
//! Pass [`TokioHandle::default`] to build one multi-thread Tokio runtime shared
//! by all Arty workers:
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
//! This crate owns the runtime until Arty stops. It uses
//! [`tokio::runtime::Builder::new_multi_thread`] and Tokio's default worker
//! count, with all Tokio drivers enabled.
//!
//! # Using a Tokio-based library
//!
//! After initialization, an Arty task can await async code that requires a
//! Tokio runtime context:
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use arty::task::Builtins;
//! use arty_tokio::TokioHandle;
//!
//! async fn call_tokio_based_library() -> u64 {
//!     tokio::time::sleep(Duration::from_millis(1)).await;
//!     42
//! }
//!
//! # #[arty::main]
//! # async fn main(cx: Builtins) -> Result<(), ohno::AppError> {
//! arty_tokio::init(&cx, TokioHandle::default()).await?;
//! assert_eq!(call_tokio_based_library().await, 42);
//! # Ok(())
//! # }
//! ```
//!
//! The complete runnable version is in the `tokio_library` example:
//! `cargo run -p arty_tokio --example tokio_library`.
//!
//! # Initialization failure
//!
//! Initialization prepares every worker before it commits. If any worker
//! fails, [`init`] removes the contexts created by that attempt. Canceling
//! [`init`] starts the same cleanup. A later call can retry after cleanup
//! finishes. If this crate created a runtime for the failed attempt, it starts
//! non-waiting Tokio shutdown during rollback.
//!
//! # External runtime
//!
//! Pass a multi-thread Tokio [`Handle`] to use a runtime owned by the
//! application:
//!
//! ```
//! use arty::task::Builtins;
//! use tokio::runtime::Builder;
//!
//! # #[arty::main]
//! # async fn main(cx: Builtins) -> Result<(), ohno::AppError> {
//! let tokio = Builder::new_multi_thread().enable_all().build()?;
//! arty_tokio::init(&cx, tokio.handle()).await?;
//! let value = tokio::spawn(async { 42 })
//!     .await
//!     .expect("Tokio task must complete");
//! assert_eq!(value, 42);
//! # Ok(())
//! # }
//! ```
//!
//! This crate stores only a clone of the handle. Keep the runtime alive while
//! Arty uses Tokio. [`init`] rejects current-thread Tokio runtimes.
//!
//! # Compatibility
//!
//! After initialization, Arty tasks on every worker can use:
//!
//! - [`tokio::runtime::Handle::current`] and
//!   [`tokio::runtime::Handle::try_current`];
//! - [`tokio::spawn`] and [`tokio::task::spawn_blocking`] submission;
//! - Tokio timers, networking, and I/O resources with enabled drivers;
//! - directly awaiting Tokio-based futures from Arty tasks.
//!
//! Tokio tasks run on Tokio workers and can move between them. They do not
//! receive [`arty::task::Builtins`] or keep Arty thread-aware state. They are
//! not tied to the Arty worker that submitted them. Tokio
//! [`JoinHandle`](tokio::task::JoinHandle) and
//! [`JoinError`](tokio::task::JoinError) keep their normal behavior.
//!
//! This crate does not support:
//!
//! - current-thread Tokio runtimes and implicit `LocalSet` / `spawn_local`
//!   integration;
//! - [`tokio::task::block_in_place`] from an Arty worker;
//! - `Runtime::block_on` or `Handle::block_on` from an Arty worker;
//! - Tokio cooperative scheduling or Tokio task-local context when Arty polls
//!   a Tokio future directly. If a library needs them, run its root future
//!   with [`tokio::spawn`];
//! - Tokio context during final Arty timer or worker teardown callbacks.
//!
//! The `tokio_drivers` example uses a timer and a loopback TCP connection:
//! `cargo run -p arty_tokio --example tokio_drivers`.
//!
//! # Shutdown
//!
//! Installer tasks stay parked until Arty stops. Arty drops each installer on
//! its worker, which leaves the Tokio context. If this crate owns the runtime,
//! it then shuts down the runtime. Tokio waits for blocking work during normal
//! runtime drop, so long [`tokio::task::spawn_blocking`] work can delay Arty
//! shutdown.
//!
//! If Arty shutdown completes on a Tokio runtime thread, this crate starts
//! background Tokio shutdown instead. This prevents that thread from waiting
//! for itself. In this case, Arty shutdown does not wait for outstanding Tokio
//! blocking work.

use tokio::runtime::Handle;

mod error;
mod initialization;

pub use error::InitError;

/// Selects the Tokio runtime used by [`init`].
///
/// Use [`Default`] for a runtime that this crate builds and owns. To use an
/// application-owned multi-thread runtime, convert its Tokio [`Handle`].
/// The value is opaque and its [`Debug`] output does not expose runtime details.
///
/// Owned runtimes are not accepted:
///
/// ```compile_fail
/// use arty_tokio::TokioHandle;
///
/// let runtime = tokio::runtime::Builder::new_multi_thread().build().unwrap();
/// let _: TokioHandle = runtime.into();
/// ```
#[derive(derive_more::Debug)]
pub struct TokioHandle {
    #[debug(ignore)]
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

impl From<&Handle> for TokioHandle {
    fn from(handle: &Handle) -> Self {
        handle.clone().into()
    }
}

/// Enters one Tokio runtime context on every worker in the current Arty runtime.
///
/// Pass [`TokioHandle::default`] to create a runtime. To use an existing
/// multi-thread Tokio runtime, pass its owned or borrowed [`Handle`]. The
/// function returns after every fixed worker enters the same context. The
/// contexts remain until Arty stops.
///
/// Calling this function again on the same Arty runtime returns an error and
/// leaves the existing contexts in place.
///
/// # Errors
///
/// Returns [`InitError`] if initialization fails on any worker. Before
/// returning an error, the function removes every context created by this
/// attempt. Canceling the future starts the same cleanup without waiting for
/// it to finish.
pub async fn init(cx: &arty::task::Builtins, tokio: impl Into<TokioHandle>) -> Result<(), InitError> {
    initialization::initialize_all_workers(cx, tokio.into()).await
}
