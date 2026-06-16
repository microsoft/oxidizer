// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/favicon.ico")]

//! Adapt a [`fetch::HttpClient`] into an Azure SDK HTTP transport.
//!
//! The Azure SDK abstracts its HTTP transport behind the
//! [`azure_core::http::HttpClient`] trait. [`HttpClient`] implements it on top
//! of a `fetch` client, so Azure SDK pipelines run over `fetch` and benefit
//! from its resilience and observability.
//!
//! To run the Azure SDK on an `anyspawn`-backed async runtime, see the
//! `anyspawn_azure` crate.
//!
//! # Example
//!
//! ```
//! use azure_core::http::{ClientOptions, Transport};
//! use fetch_azure::HttpClient;
//!
//! # fn example(client: fetch::HttpClient) {
//! // Wire a `fetch` client in as the transport for an Azure SDK client.
//! let transport = Transport::new(HttpClient::from(client).into());
//! let options = ClientOptions {
//!     transport: Some(transport),
//!     ..Default::default()
//! };
//! # let _ = options;
//! # }
//! ```

mod client;

pub use client::HttpClient;
