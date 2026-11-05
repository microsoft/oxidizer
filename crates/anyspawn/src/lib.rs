// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

//! A task spawner for Arty and Tokio.
//!
//! [`Spawner`] provides the same task-spawning API for both runtimes without
//! runtime-specific type parameters. Enable `arty` or `tokio` to construct a
//! spawner. Both features can be enabled together.
//! Without either feature, the public types remain available for adapter crates,
//! but no spawner can be constructed.
//!
//! # Quick Start
//!
//! ## Using Arty
//!
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "arty")] {
//! # (|| {
//! use anyspawn::Spawner;
//! use arty::runtime::Runtime;
//!
//! let runtime = Runtime::new()?;
//! # runtime.scheduler().block_on(async |cx| {
//! let spawner = Spawner::new_arty(cx.scheduler().clone());
//! let result = spawner.spawn(async { 1 + 1 }).await?;
//! assert_eq!(result, 2);
//! # Ok::<(), anyspawn::JoinError>(())
//! # })??;
//! runtime.stop()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # })().unwrap();
//! # }
//! # }
//! ```
//!
//! ## Using Tokio
//!
//! ```rust
//! # #[cfg(feature = "tokio")]
//! # #[tokio::main]
//! # async fn main() -> Result<(), anyspawn::JoinError> {
//! use anyspawn::Spawner;
//!
//! let spawner = Spawner::new_tokio(None);
//! let result = spawner.spawn(async { 1 + 1 }).await?;
//! assert_eq!(result, 2);
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "tokio"))]
//! # fn main() {}
//! ```
//!
//! `Spawner::new_tokio` accepts a runtime handle, `Some(handle)`, or `None`.
//! `None` selects the runtime active at each spawn call.
//! `Spawner::new_arty` takes ownership of a scheduler.
//! These constructors do not enable enrichment transfer.
//! Use `Spawner::builder_tokio` or `Spawner::builder_arty` to configure
//! a spawner through [`SpawnerBuilder`].
//!
//! Awaiting a [`JoinHandle`] returns `Result<T, JoinError>`. A task panic,
//! cancellation, or shutdown rejection becomes an error rather than another panic
//! in the joining task. The runtime's panic policy still applies.
//! A task returning `Result<T, E>` produces `Result<Result<T, E>, JoinError>`.
//!
//! # Thread-Aware Support
//!
//! `Spawner` implements [`ThreadAware`](thread_aware::ThreadAware).
//! Arty spawners follow their scheduler's worker association during relocation.
//! Tokio spawners ignore relocation and keep the same runtime selection.
//!
//! # Observed Enrichment
//!
//! Enrichment is context attached to events emitted through an `observed::Sink`.
//! With the `observed` feature and a runtime feature, call `observed(sink)`
//! on the [`SpawnerBuilder`] to transfer context to spawned work.
//! Each spawn call captures the supplied sink's context. Async tasks restore it
//! on each poll, and blocking tasks restore it for the closure call.
//!
//! `spawn_anywhere` also restores context while its factory creates the future.
//! Arty still relocates the task data before calling the factory.
//! Spawners without a sink do not transfer context, even when `observed` is enabled.
//!
//! ```rust
//! # #[cfg(all(feature = "tokio", feature = "observed"))]
//! # #[tokio::main]
//! # async fn main() -> Result<(), anyspawn::JoinError> {
//! use anyspawn::Spawner;
//! use observed::enrichment::EnrichFutureExt;
//! use observed::{Enrichment, Sink, event};
//!
//! #[derive(Clone, Debug, Enrichment)]
//! struct Request {
//!     #[unredacted]
//!     request_id: u64,
//! }
//!
//! #[event("task.started")]
//! #[info]
//! struct TaskStarted;
//!
//! let sink = Sink::builder("app").build();
//! let spawner = Spawner::builder_tokio(None).observed(sink.clone()).build();
//! async {
//!     // Events emitted through this sink in the task include request_id.
//!     let task_sink = sink.clone();
//!     spawner
//!         .spawn(async move {
//!             observed::emit!(&task_sink, TaskStarted);
//!         })
//!         .await?;
//!     Ok::<(), anyspawn::JoinError>(())
//! }
//! .enrich(&sink, Request { request_id: 42 })
//! .await?;
//! # Ok(())
//! # }
//! # #[cfg(not(all(feature = "tokio", feature = "observed")))]
//! # fn main() {}
//! ```
//!
//! # Features
//!
//! - `arty`: Enables the Arty constructors on `Spawner`.
//! - `tokio`: Enables the Tokio constructors on `Spawner`.
//! - `observed`: Enables `SpawnerBuilder::observed`, which accepts an `observed::Sink`.
//!   Combine it with `arty` or `tokio`. It does not select a runtime.
//! - `seismograph`: Enables instrumentation in the optional `observed` dependency.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/anyspawn/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/anyspawn/favicon.ico")]

mod handle;
mod join_error;
mod spawner;

pub use handle::JoinHandle;
pub use join_error::JoinError;
pub use spawner::{Spawner, SpawnerBuilder};
