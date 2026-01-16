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
//! ```rust,ignore
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
//! - `custom`: Enables the [`Spawner::custom`] constructor

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

#[cfg(not(any(feature = "tokio", feature = "custom")))]
compile_error!("at least one of the `tokio` or `custom` features must be enabled");

#[cfg(feature = "custom")]
mod custom;
#[cfg(any(feature = "tokio", feature = "custom"))]
mod handle;
#[cfg(any(feature = "tokio", feature = "custom"))]
mod spawner;

#[cfg(any(feature = "tokio", feature = "custom"))]
pub use handle::JoinHandle;
#[cfg(any(feature = "tokio", feature = "custom"))]
pub use spawner::Spawner;
