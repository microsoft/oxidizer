// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

//! A generic task spawner compatible with any async runtime.
//!
//! This crate provides a [`Spawner`] type that abstracts task spawning across
//! different async runtimes without generic infection.
//!
//! # Design Philosophy
//!
//! - **Concrete type**: No generics needed in your code
//! - **Simple**: Use built-in constructors or provide a closure
//! - **Flexible**: Works with any async runtime
//!
//! # Quick Start
//!
//! ## Using Tokio
//!
//! ```rust
//! use anyspawn::Spawner;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let spawner = Spawner::new_tokio();
//! let result = spawner.spawn(async { 1 + 1 }).await;
//! assert_eq!(result, 2);
//! # }
//! ```
//!
//! ## Custom Runtime
//!
//! ```rust,ignore
//! use anyspawn::Spawner;
//!
//! let spawner = Spawner::new_custom("threadpool", |fut| {
//!     std::thread::spawn(move || futures::executor::block_on(fut));
//! });
//!
//! // Returns a JoinHandle that can be awaited or dropped
//! let handle = spawner.spawn(async { 42 });
//! ```
//!
//! # Thread-Aware Support
//!
//! `Spawner` implements [`ThreadAware`](thread_aware::ThreadAware) and supports
//! per-core isolation via [`Spawner::new_thread_aware`], enabling
//! contention-free, NUMA-friendly task dispatch. See the
//! [thread-aware section on `Spawner`](Spawner#thread-aware-support) for
//! details and examples.
//!
//! # Features
//!
//! - `tokio` (default): Enables the [`Spawner::new_tokio`] constructor
//! - `custom`: Enables [`Spawner::new_custom`] and [`CustomSpawnerBuilder`]

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/anyspawn/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/anyspawn/favicon.ico")]

#[cfg(feature = "custom")]
mod builder;
#[cfg(feature = "custom")]
mod custom;
mod handle;
mod spawner;

#[cfg(feature = "custom")]
pub use builder::CustomSpawnerBuilder;
#[cfg(feature = "custom")]
pub use custom::BoxedFuture;
pub use handle::JoinHandle;
pub use spawner::Spawner;
