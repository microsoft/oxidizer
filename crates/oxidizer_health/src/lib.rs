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
//! [HTTP helpers](https://docs.rs/oxidizer_health/latest/oxidizer_health/http/) expose readiness
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
//! Keep the component and its publishers alive while the subsystem participates in health reporting.
//! Dropping a publisher removes its contribution; dropping the last [`Component`] handle unregisters
//! the component. A component without publishers is `Unhealthy`, as is an [`Aggregator`] without components.
//!
//! # Custom health checks and HTTP readiness
//!
//! A custom check measures a dependency or application condition and publishes the result.
//! The application invokes the check at startup, periodically, or after relevant operations.
//! The HTTP helper reads the latest aggregate for each request.
//!
//! This complete example checks TCP connectivity to a backend and invokes the Tower readiness
//! handler with a probe request. A local TCP listener supplies the backend so the example can run
//! independently. It uses the `http` feature and Tower with the `util` feature for `oneshot`.
//! Run the async code in an application executor.
//!
//! ```
//! # fn main() {
//! # #[cfg(feature = "http")] {
//! # testing_aids::init_tracing!();
//! use std::net::{SocketAddr, TcpListener, TcpStream};
//! use std::time::Duration;
//!
//! use http::{Request, StatusCode};
//! use oxidizer_health::http::service;
//! use oxidizer_health::{Aggregator, Health, Publisher};
//! use tower::ServiceExt;
//!
//! fn check_backend(address: SocketAddr, publisher: &mut Publisher) {
//!     match TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
//!         Ok(_) => publisher.publish(Health::Healthy, [("endpoint", address.to_string())]),
//!         Err(error) => publisher.publish(Health::Unhealthy, [("error", error.to_string())]),
//!     }
//! }
//!
//! # futures::executor::block_on(async {
//! let listener = TcpListener::bind("127.0.0.1:0").unwrap();
//! let address = listener.local_addr().unwrap();
//! let aggregator = Aggregator::new();
//! let backend = aggregator.component("backend");
//! let mut monitor = aggregator.monitor();
//!
//! let mut publisher = backend.publisher();
//! check_backend(address, &mut publisher);
//! while monitor.state() != publisher.signal().state() {
//!     monitor.changed().await;
//! }
//!
//! let handler = service(aggregator);
//! let request = Request::builder().uri("/health/ready").body(()).unwrap();
//! let response = handler.oneshot(request).await.unwrap();
//! assert_eq!(response.status(), StatusCode::OK);
//! assert_eq!(response.body(), "true");
//! # });
//! # }
//! # }
//! ```
//!
//! The check measures TCP connectivity; an application-level check can instead perform a protocol
//! exchange or inspect application state. Reuse the publisher for subsequent checks so failures and
//! recoveries replace its previous signal.
//!
//! Both HTTP helpers return HTTP 200 with JSON `true` for `Healthy`, and HTTP 503 with JSON `false`
//! for `Degraded` or `Unhealthy`. Responses use `Content-Type: application/json` and
//! `Cache-Control: no-store`. Register the handler on a readiness route in your HTTP server;
//! the example invokes it directly without starting a server.
//!
//! A new publisher starts `Healthy`. For dependencies that must pass a check before admitting
//! traffic, register their components on a new [`Aggregator`] before serving probes. Keep each component
//! empty until its first successful check, then create and retain its publisher for later results.
//! This keeps the component `Unhealthy` during startup.
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
//! - `http` enables the [Tower readiness service](https://docs.rs/oxidizer_health/latest/oxidizer_health/http/fn.service.html).
//! - `request-handler` enables the [`http_extensions` readiness handler](https://docs.rs/oxidizer_health/latest/oxidizer_health/http/fn.request_handler.html).
//!
//! No features are enabled by default. Collection, aggregation, change notifications, and reports
//! are available in the default configuration.
//!
//! # Additional documentation
//!
//! - Register a [`Component`] with [`Aggregator::component`] and report changes with [`Publisher::publish`].
//! - Get a read-only [`Monitor`] with [`Aggregator::monitor`] to observe changes independently of publishers.
//! - Select diagnostic signals with [`Filter`] and inspect their attributes and publisher counts with [`Report::signals`].
//! - Serve readiness with one of the [HTTP adapters](https://docs.rs/oxidizer_health/latest/oxidizer_health/http/).

mod aggregator;
mod attribute;
mod component;
mod health;
#[cfg(any(test, feature = "http", feature = "request-handler"))]
pub mod http;
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
