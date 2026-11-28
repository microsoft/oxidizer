// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

//! Readiness adapters for [`health`](https://docs.rs/process_health/latest/health/).
//!
//! The adapter reads a
//! [`health::Monitor`](https://docs.rs/process_health/latest/health/?search=Monitor) for each
//! request. A healthy process returns status `200 OK` with the JSON body `true`. A degraded or
//! unhealthy process returns `503 Service Unavailable` with the JSON body `false`. Responses
//! contain no component details and disable caching.
//!
//! # Features
//!
//! - `http-readiness-tower` enables the Tower readiness service.
//!
//! No features are enabled by default.

#[cfg(any(test, feature = "http-readiness-tower"))]
pub mod http;
