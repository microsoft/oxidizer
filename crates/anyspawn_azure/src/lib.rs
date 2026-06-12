// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/anyspawn_azure/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/anyspawn_azure/favicon.ico")]

//! Bundle [`anyspawn`] and [`tick`] as Azure SDK runtime abstractions.
//!
//! The Azure SDK abstracts its task spawning, sleeping, and yielding behind the
//! [`typespec_client_core::async_runtime::AsyncRuntime`] trait, and the process
//! execution that developer credentials rely on behind the `azure_identity::Executor`
//! trait. This crate adapts those primitives to both:
//!
//! - [`Runtime`] implements [`typespec_client_core::async_runtime::AsyncRuntime`] on top of
//!   an [`anyspawn::Spawner`] (spawning) and a [`tick::Clock`] (sleeping).
//! - With the `azure-identity` feature, [`Runtime`] also implements
//!   `azure_identity::Executor`, running credential commands on the
//!   [`anyspawn::Spawner`].
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//!
//! use anyspawn::Spawner;
//! use anyspawn_azure::Runtime;
//! use tick::Clock;
//! use typespec_client_core::async_runtime::{AsyncRuntime, set_async_runtime};
//!
//! // Install an `anyspawn`-backed async runtime (sleeping on a `tick::Clock`).
//! fn install_runtime(spawner: Spawner, clock: Clock) {
//!     let runtime: Arc<dyn AsyncRuntime> = Runtime::new(spawner, clock).into();
//!     let _ = set_async_runtime(runtime);
//! }
//! # let _ = install_runtime;
//! ```

mod runtime;

pub use runtime::Runtime;
