// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![deny(missing_docs)]

//! Health signal collection, aggregation, and HTTP readiness helpers for Rust services.
//!
//! [`Publisher`]s collect application-defined health signals into named [`Component`]s.
//! Each component aggregates its publishers, and an [`Aggregator`] combines the components into
//! an overall service state: [`Healthy`](Health::Healthy), [`Degraded`](Health::Degraded), or
//! [`Unhealthy`](Health::Unhealthy). At each level, the worst state wins.
//! [Reports](Report) retain diagnostic attributes and publisher counts, while the optional
//! [Kubernetes HTTP helpers](https://docs.rs/oxidizer_health/latest/oxidizer_health/kubernetes_http/) expose readiness
//! through Tower or [`http_extensions`](https://docs.rs/http_extensions).
//!
//! # Quick start
//!
//! Create an [`Aggregator`], register an application-defined component, and publish a health signal.
//! [`Aggregator::component`] creates the named component on first use; subsequent calls with the
//! same name return a handle to that component. The name identifies a subsystem, not a built-in check.
//!
//! ```
//! # testing_aids::init_tracing!();
//! use oxidizer_health::{Aggregator, Health};
//!
//! # futures::executor::block_on(async {
//! let aggregator = Aggregator::new();
//! let backend = aggregator.component("backend");
//! let mut monitor = aggregator.monitor();
//! let mut publisher = backend.publisher();
//!
//! publisher.degraded("connection pool exhausted");
//! while monitor.state() != Health::Degraded {
//!     monitor.changed().await;
//! }
//! assert_eq!(monitor.state(), Health::Degraded);
//! # });
//! ```
//!
//! Publishing updates the local [`Signal`] immediately; aggregation runs in the background.
//! [`Monitor::changed`] notifies observers when state or report data may have changed.
//! Re-read the current state after each notification, as updates can coalesce.
//!
//! Dropping a publisher removes its contribution; dropping the last [`Component`] handle unregisters
//! the component. A component without publishers is `Unhealthy`, as is an [`Aggregator`] without components.
//!
//! # Diagnose a change
//!
//! A read-only [`Monitor`] provides component reports independently of publishers.
//! This example publishes a problem, waits for aggregation, and prints its diagnostic signal:
//!
//! ```
//! # testing_aids::init_tracing!();
//! use oxidizer_health::{Aggregator, Filter, Health};
//!
//! # futures::executor::block_on(async {
//! let aggregator = Aggregator::new();
//! let backend = aggregator.component("backend");
//! let mut monitor = aggregator.monitor();
//! let mut publisher = backend.publisher();
//! publisher.degraded("connection pool exhausted");
//! while monitor.state() != Health::Degraded {
//!     monitor.changed().await;
//! }
//!
//! for report in monitor.reports(Filter::DEGRADED | Filter::UNHEALTHY).await {
//!     println!("{report}");
//!     for state in [Health::Degraded, Health::Unhealthy] {
//!         for (signal, count) in report.signals(state) {
//!             println!("  {signal} ({count} publishers)");
//!         }
//!     }
//! }
//! # });
//! ```
//!
//! Reports group identical signals and count their publishers. [`Filter`] selects diagnostic
//! signals; all registered components and their aggregate states remain in the reports.
//! An empty component has an `Unhealthy` summary with no signals. For continuous observation,
//! read the reports, await [`Monitor::changed`], and repeat.
//!
//! # Features
//!
//! - `http` enables the [Tower readiness service](https://docs.rs/oxidizer_health/latest/oxidizer_health/kubernetes_http/fn.service.html).
//! - `request-handler` enables the [`http_extensions` readiness handler](https://docs.rs/oxidizer_health/latest/oxidizer_health/kubernetes_http/fn.request_handler.html).
//!
//! No features are enabled by default. Collection, aggregation, change notifications, and reports
//! are available in the default configuration.
//!
//! # Additional documentation
//!
//! - Register a [`Component`] with [`Aggregator::component`] and report changes with [`Publisher::publish`].
//! - Get a read-only [`Monitor`] with [`Aggregator::monitor`] to observe changes independently of publishers.
//! - Select diagnostic signals with [`Filter`] and inspect their attributes and publisher counts with [`Report::signals`].
//! - Serve readiness with one of the [Kubernetes HTTP adapters](https://docs.rs/oxidizer_health/latest/oxidizer_health/kubernetes_http/).

mod aggregator;
mod attribute;
mod component;
mod health;
#[cfg(any(test, feature = "http", feature = "request-handler"))]
pub mod kubernetes_http;
mod report;
mod signal;
mod worker;

pub use aggregator::{Aggregator, Monitor};
pub use attribute::{Attribute, AttributeString, AttributeValue};
pub use component::{Component, Publisher};
pub use health::Health;
pub use report::{Filter, Report, Reports, Signals};
pub use signal::Signal;

#[cfg(test)]
testing_aids::init_tracing!();

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Aggregator: Send, Sync, Clone);
    assert_impl_all!(Monitor: Send, Sync, Clone);
    assert_impl_all!(Component: Send, Sync, Clone);
    assert_impl_all!(Publisher: Send, Sync, Clone);
    assert_impl_all!(Signal: Send, Sync, Clone);
    assert_impl_all!(Report: Send, Sync, Clone);
}
