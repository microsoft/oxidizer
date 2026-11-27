// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

//! Readiness adapters for [`health`].
//!
//! Both adapters read a [`health::Monitor`] for each request. A healthy process returns
//! status `200 OK` with the JSON body `true`. A degraded or unhealthy process returns
//! `503 Service Unavailable` with the JSON body `false`. Responses contain no component details
//! and disable caching.
//!
//! # Features
//!
//! - `http` enables the Tower readiness service.
//! - `request-handler` enables the
//!   [`http_extensions`](https://docs.rs/http_extensions) request handler.
//!
//! No features are enabled by default.

#[cfg(any(test, feature = "http", feature = "request-handler"))]
pub mod kubernetes_http;
