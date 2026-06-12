// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/favicon.ico")]

//! Adapt a [`fetch::HttpClient`] into an Azure SDK HTTP transport.
//!
//! The Azure SDK abstracts its HTTP transport behind the
//! [`azure_core::http::HttpClient`] trait. [`AzureHttpClient`] implements that
//! trait on top of a [`fetch::HttpClient`], so Azure SDK pipelines run over
//! `fetch` and benefit from its resilience and observability.
//!
//! To run the Azure SDK on an [`anyspawn`]-backed async runtime, see the
//! `anyspawn_azure` crate.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//!
//! use azure_core::http::HttpClient;
//! use fetch::HttpClient as FetchClient;
//! use fetch_azure::AzureHttpClient;
//!
//! // Adapt a `fetch` client into an Azure SDK transport.
//! fn transport(client: FetchClient) -> Arc<dyn HttpClient> {
//!     AzureHttpClient::from(client).into()
//! }
//! # let _ = transport;
//! ```

mod client;

pub use client::AzureHttpClient;
