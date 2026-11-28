// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]

//! Readiness adapters for [`health`](https://docs.rs/process_health/latest/health/).
//!
//! The TCP adapter listens on a dedicated port after it observes
//! [`health::Health::Healthy`](https://docs.rs/process_health/latest/health/?search=Health::Healthy).
//! It closes the port after it observes another state. The HTTP adapter reads a
//! [`health::Monitor`](https://docs.rs/process_health/latest/health/?search=Monitor) for each
//! request. A healthy process returns status `200 OK` with the JSON body `true`. A degraded or
//! unhealthy process returns `503 Service Unavailable` with the JSON body `false`. Responses
//! contain no component details and disable caching.
//!
//! # Features
//!
//! - `http-readiness-tower` enables the Tower readiness service.
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
//!     path: /health/ready
//!     port: 8080
//! ```

#[cfg(any(test, feature = "http-readiness-tower"))]
pub mod http;

#[cfg(feature = "tcp")]
#[cfg_attr(docsrs, doc(cfg(feature = "tcp")))]
pub mod kubernetes_tcp;
