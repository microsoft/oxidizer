// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

//! Kubernetes readiness adapters for [`health`].
//!
//! The TCP adapter listens on a dedicated port only while the aggregate state is
//! [`health::Health::Healthy`]. The HTTP adapters read a [`health::Monitor`] for each request.
//! They return `200 OK` with JSON `true` for a healthy process. They return
//! `503 Service Unavailable` with JSON `false` for any other state.
//!
//! # Features
//!
//! - `http` enables the Tower readiness service.
//! - `request-handler` enables the
//!   [`http_extensions`](https://docs.rs/http_extensions) request handler.
//! - `tcp` enables the TCP readiness server.
//!
//! No features are enabled by default.
//!
//! # Kubernetes configuration
//!
//! Point a TCP probe at a dedicated readiness port:
//!
//! ```yaml
//! readinessProbe:
//!   tcpSocket:
//!     port: 8081
//! ```
//!
//! Point an HTTP probe at the route that contains an HTTP adapter:
//!
//! ```yaml
//! readinessProbe:
//!   httpGet:
//!     path: /ready
//!     port: 8080
//! ```

#[cfg(any(test, feature = "http", feature = "request-handler"))]
pub mod kubernetes_http;

#[cfg(feature = "tcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "tcp")))]
pub mod kubernetes_tcp;
