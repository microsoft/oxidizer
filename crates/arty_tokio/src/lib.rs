// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Lets [`arty`] tasks use Tokio-based libraries.
//!
//! Use [`TokioCompatExt::tokio_compat`] from an async main function to create a
//! Tokio runtime and enter its context on every live Arty worker:
//!
//! ```
//! use arty::task::Context;
//! use arty_tokio::TokioCompatExt as _;
//!
//! # #[arty::main]
//! # async fn main(cx: Context) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! cx.control().tokio_compat();
//! let value = tokio::spawn(async { 42 }).await?;
//! assert_eq!(value, 42);
//! # Ok(())
//! # }
//! ```
//!
//! Use [`TokioCompatExt::tokio_compat_with`] to configure workers before the
//! runtime starts:
//!
//! ```
//! use arty::runtime::{Runtime, WorkersPolicy};
//! use arty_tokio::TokioCompatExt as _;
//! use tokio::runtime::Builder;
//!
//! let tokio = Builder::new_multi_thread().enable_all().build()?;
//! let runtime = Runtime::builder()
//!     .workers(WorkersPolicy::exactly(2))
//!     .tokio_compat_with(tokio.handle())
//!     .build()?;
//! assert_eq!(runtime.scheduler().block_on(async |_| 42)?, 42);
//! runtime.stop()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The first initialization wins on each worker. Later calls to
//! [`TokioCompatExt::tokio_compat_with`] do nothing on that worker, even with a
//! different Tokio runtime. They do not replace an existing Tokio context.
//! Builder registrations run in registration order. Through runtime control,
//! setup runs immediately on the calling worker and is queued on other workers.
//!
//! If you supply a Tokio handle, the application owns its runtime. Keep that
//! runtime alive while Arty tasks use Tokio. Each Arty worker stores a clone of
//! the handle.
//!
//! If you supply an owned runtime or use [`TokioCompatExt::tokio_compat`], Arty
//! drops the owned Tokio runtime after
//! [`Runtime::stop`](arty::runtime::Runtime::stop) or runtime destruction
//! finishes waiting for the workers.
//!
//! This crate does not support current-thread Tokio runtimes. If you supply
//! one, you are responsible for full interoperability with Arty.
//!
//! After setup, Arty tasks can use Tokio task spawning and libraries that
//! require a Tokio runtime context. Timers require the Tokio `time` feature and
//! an enabled time driver. Networking and I/O resources require the applicable
//! Tokio features and an enabled I/O driver. Configure these drivers when you
//! build an explicit Tokio runtime.
//! Tokio tasks run on Tokio workers. They do not receive
//! [`arty::task::Context`] or keep Arty thread-aware state.
//!
//! Do not call `tokio::task::block_in_place`, `Runtime::block_on`, or
//! `Handle::block_on` from an Arty worker. `LocalSet` and `spawn_local` are not
//! integrated with Arty.

use tokio::runtime::{Handle, Runtime};

mod extensions;

pub use extensions::TokioCompatExt;

/// Selects the Tokio runtime that Arty workers enter.
///
/// Convert an owned or borrowed Tokio [`Handle`] from an application-owned
/// runtime. The value is opaque and its [`Debug`] output does not
/// expose runtime details.
///
/// Create a runtime explicitly to handle creation failure or configure its
/// drivers, then convert it to let Arty keep it alive:
///
/// ```
/// use arty_tokio::TokioHandle;
///
/// let runtime = tokio::runtime::Builder::new_multi_thread().build()?;
/// let handle = TokioHandle::from(runtime);
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(derive_more::Debug)]
pub struct TokioHandle {
    #[debug(ignore)]
    handle: Handle,
    #[debug(ignore)]
    owned_runtime: Option<Runtime>,
}

impl From<Runtime> for TokioHandle {
    fn from(runtime: Runtime) -> Self {
        Self {
            handle: runtime.handle().clone(),
            owned_runtime: Some(runtime),
        }
    }
}

impl From<Handle> for TokioHandle {
    fn from(handle: Handle) -> Self {
        Self {
            handle,
            owned_runtime: None,
        }
    }
}

impl From<&Handle> for TokioHandle {
    fn from(handle: &Handle) -> Self {
        handle.clone().into()
    }
}
