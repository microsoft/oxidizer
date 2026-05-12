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
//! - **Simple**: Use built-in constructors or implement [`SpawnCustom`]
//! - **Layered**: Compose middleware closures via [`CustomSpawnerBuilder`]
//! - **Flexible**: Works with any async runtime
//!
//! # Quick Start
//!
//! ## Using Tokio
//!
//! ```rust
//! # #[cfg(feature = "tokio")]
//! # #[tokio::main]
//! # async fn main() {
//! use anyspawn::Spawner;
//!
//! let spawner = Spawner::new_tokio();
//! let result = spawner.spawn(async { 1 + 1 }).await;
//! assert_eq!(result, 2);
//! # }
//! # #[cfg(not(feature = "tokio"))]
//! # fn main() {}
//! ```
//!
//! # Thread-Aware Support
//!
//! `Spawner` implements [`ThreadAware`](thread_aware::ThreadAware) and supports
//! per-core isolation via custom [`SpawnCustom`] implementations, enabling
//! contention-free, NUMA-friendly task dispatch.
//!
//! # Features
//!
//! - `tokio`: Enables the [`Spawner::new_tokio`] and
//!   [`Spawner::new_tokio_with_handle`] constructors

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/anyspawn/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/anyspawn/favicon.ico")]

mod builder;
mod custom;
mod handle;
mod spawner;

pub use builder::CustomSpawnerBuilder;
pub use custom::{BoxedFuture, SpawnCustom};
pub use handle::JoinHandle;
pub use spawner::Spawner;
pub use thread_aware::closure::ThreadAwareAsyncFnOnce;
