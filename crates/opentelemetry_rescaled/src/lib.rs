// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::panic,
        clippy::float_cmp,
        reason = "unwrap, panic, and exact float comparisons keep tests concise and readable"
    )
)]

//! Wraps an inner OpenTelemetry meter provider to transparently emit *rescaled*
//! side-by-side copies of selected instruments.
//!
//! For a chosen instrument in a chosen instrumentation scope, this layer creates
//! a second instrument whose measurements are the original values multiplied by a
//! fixed factor. For example, a `http.client.request.duration` instrument that
//! records seconds can gain a `http.client.request.duration.millis` sidecar that
//! records the same measurements multiplied by `1000.0`.
//!
//! The rescaling is invisible to instrument users — they interact only with their
//! original instrument — and the inner provider simply sees two independently
//! registered instruments.
//!
//! # Quick start
//!
//! ```
//! use opentelemetry::metrics::MeterProvider;
//! use opentelemetry_rescaled::RescaledMetrics;
//!
//! // Any `MeterProvider` works as the inner provider.
//! let inner = opentelemetry::metrics::noop::NoopMeterProvider::new();
//!
//! let outer = RescaledMetrics::builder(inner)
//!     .scope("my_scope_name", |scope| {
//!         // source name, target name, target unit (mandatory), factor
//!         scope.rescale(
//!             "http.client.request.duration",
//!             "http.client.request.duration.millis",
//!             "ms",
//!             1000.0,
//!         );
//!     })
//!     .build();
//!
//! // `outer` is itself a `MeterProvider`.
//! let meter = outer.meter("my_scope_name");
//! let histogram = meter.f64_histogram("http.client.request.duration").build();
//! histogram.record(1.5, &[]); // recorded as 1.5 s and, on the sidecar, 1500 ms
//! ```
//!
//! See [`docs/DESIGN.md`](https://github.com/microsoft/oxidizer/blob/main/crates/opentelemetry_rescaled/docs/DESIGN.md)
//! for the architecture and the resolved design decisions.

mod config;
mod instruments;
mod provider;
mod rescale;

pub use config::ScopeConfigurator;
pub use provider::{RescaledMetrics, RescaledMetricsBuilder};
