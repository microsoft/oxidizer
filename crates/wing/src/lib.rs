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
//! # #[tokio::main]
//! # async fn main() {
//! let spawner = TokioSpawner;
//!
//! // Spawn a fire-and-forget task
//! spawner.spawn(async {
//!     println!("Task running!");
//! });
//! # }
//! ```
//!
//! ## Custom Implementation
//!
//! ```rust
//! use wing::Spawner;
//! use std::future::Future;
//!
//! #[derive(Clone)]
//! struct MySpawner;
//!
//! impl Spawner for MySpawner {
//!     fn spawn<T>(&self, work: T)
//!     where
//!         T: Future<Output = ()> + Send + 'static,
//!     {
//!         // Your implementation here
//!         std::thread::spawn(move || futures::executor::block_on(work));
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
