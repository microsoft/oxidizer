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
//! let spawner = Spawner::new_custom(|fut| {
//!     std::thread::spawn(move || futures::executor::block_on(fut));
//! });
//!
//! // Returns a JoinHandle that can be awaited or dropped
//! let handle = spawner.spawn(async { 42 });
//! ```
//!
//! # Features
//!
//! - `tokio` (default): Enables the [`Spawner::new_tokio`] constructor
//! - `custom`: Enables the [`Spawner::new_custom`] constructor

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/anyspawn/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/anyspawn/favicon.ico")]

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
