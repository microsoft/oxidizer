// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

//! Async runtime abstractions
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
//! use arty::Spawner;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let spawner = Spawner::tokio();
//! let result = spawner.spawn(async { 1 + 1 }).await;
//! assert_eq!(result, 2);
//! # }
//! ```
//!
//! ## Custom Runtime
//!
//! ```rust
//! use arty::Spawner;
//!
//! let spawner = Spawner::custom(|fut| {
//!     std::thread::spawn(move || futures::executor::block_on(fut));
//! });
//!
//! // Returns a JoinHandle that can be awaited or dropped
//! let handle = spawner.spawn(async { 42 });
//! ```
//!
//! # Features
//!
//! - `tokio` (default): Enables the [`Spawner::tokio`] constructor

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

#[cfg(feature = "custom")]
mod custom;
mod handle;
mod spawner;

pub use handle::JoinHandle;
pub use spawner::Spawner;
