// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/favicon.ico")]

//! Use a [`fetch::HttpClient`] as an Azure SDK HTTP transport.
//!
//! [`HttpClient`] implements [`azure_core::http::HttpClient`] and sends requests
//! through a [`fetch::HttpClient`]. Azure SDK clients can use this adapter for
//! their HTTP requests.
//!
//! The `anyspawn_azure` crate lets Azure SDK clients run on an
//! `anyspawn`-based async runtime.
//!
//! # Example
//!
//! ```
//! use azure_core::http::{ClientOptions, Transport};
//! use fetch_azure::HttpClient;
//!
//! # fn example(client: fetch::HttpClient) {
//! // Use a `fetch` client as the Azure SDK transport.
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
