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
//! spawner.spawn(async {
//!     println!("Task running!");
//! });
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
//! spawner.spawn(async {
//!     println!("Running on custom runtime!");
//! });
//! ```
//!
//! # Features
//!
//! - `tokio` (default): Enables the [`Spawner::tokio`] constructor

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

mod spawner;

pub use spawner::Spawner;
