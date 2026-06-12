// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/favicon.ico")]

//! Bundle [`fetch`] and [`anyspawn`] as Azure SDK abstractions.
//!
//! The Azure SDK abstracts its HTTP transport behind the
//! [`azure_core::http::HttpClient`] trait and its task spawning, sleeping, and
//! yielding behind the [`azure_core::async_runtime::AsyncRuntime`] trait. This
//! crate provides adapters for both:
//!
//! - [`AzureHttpClient`] implements [`azure_core::http::HttpClient`] on top of a
//!   [`fetch::HttpClient`], so Azure SDK pipelines run over `fetch` and benefit
//!   from its resilience and observability.
//! - [`Runtime`] implements [`azure_core::async_runtime::AsyncRuntime`] on top of
//!   an [`anyspawn::Spawner`] (spawning) and a [`tick::Clock`] (sleeping).
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//!
//! use anyspawn::Spawner;
//! use azure_core::async_runtime::{AsyncRuntime, set_async_runtime};
//! use azure_core::http::HttpClient;
//! use fetch::HttpClient as FetchClient;
//! use fetch_azure::{AzureHttpClient, Runtime};
//! use tick::Clock;
//!
//! // Adapt a `fetch` client into an Azure SDK transport.
//! fn transport(client: FetchClient) -> Arc<dyn HttpClient> {
//!     AzureHttpClient::from(client).into()
//! }
//!
//! // Install an `anyspawn`-backed async runtime (sleeping on a `tick::Clock`).
//! fn install_runtime(spawner: Spawner, clock: Clock) {
//!     let runtime: Arc<dyn AsyncRuntime> = Runtime::new(spawner, clock).into();
//!     let _ = set_async_runtime(runtime);
//! }
//! # let _ = (transport, install_runtime);
//! ```

mod client;
mod runtime;

pub use client::AzureHttpClient;
pub use runtime::Runtime;
