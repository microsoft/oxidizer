// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

//! Runtime-agnostic async task spawning.
//!
//! This crate provides a [`Spawner`] enum that abstracts task spawning across
//! different async runtimes without generic infection.
//!
//! # Design Philosophy
//!
//! - **Concrete type**: No generics needed in your code
//! - **Simple**: Use built-in variants or provide a closure
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
//! let spawner = Spawner::Tokio;
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
//! let spawner = Spawner::new_custom(|fut| {
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
//! - `tokio` (default): Enables the [`Spawner::Tokio`] variant

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/arty/favicon.ico")]

mod spawner;

pub use spawner::{CustomSpawner, Spawner};
