// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

//! Trait-based async runtime abstraction for spawning tasks.
//!
//! This crate provides a [`Spawner`] trait that abstracts task spawning across different async runtimes.
//! Users can implement `Spawner` for any runtime (Tokio, oxidizer, custom runtimes).
//!
//! # Design Philosophy
//!
//! - **Trait-based**: Implement [`Spawner`] for your runtime
//! - **Simple**: Just one method to implement
//! - **Flexible**: Works with any async runtime
//!
//! # Quick Start
//!
//! ## Using Tokio
//!
//! ```rust
//! use wing::tokio::TokioSpawner;
//! use wing::Spawner;
//!
//! // TokioSpawner requires a multi-threaded runtime
//! let rt = tokio::runtime::Builder::new_multi_thread()
//!     .enable_all()
//!     .build()
//!     .unwrap();
//!
//! rt.block_on(async {
//!     let spawner = TokioSpawner;
//!
//!     // Spawn and await a task
//!     let result = spawner.spawn(async { 42 });
//!     assert_eq!(result, 42);
//! });
//! ```
//!
//! ## Custom Implementation
//!
//! ```rust
//! use wing::Spawner;
//!
//! #[derive(Clone)]
//! struct MySpawner;
//!
//! impl Spawner for MySpawner {
//!     fn spawn<T>(&self, work: impl Future<Output = T> + Send + 'static) -> T
//!     where
//!         T: Send + 'static,
//!     {
//!         // Your implementation here
//!         # futures::executor::block_on(work)
//!     }
//! }
//! ```
//!
//! # Features
//!
//! - `tokio` (default): Enables [`tokio::TokioSpawner`] implementation

#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/wing/logo.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/wing/favicon.ico"
)]

mod spawner;

#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
pub mod tokio;

pub use spawner::Spawner;
