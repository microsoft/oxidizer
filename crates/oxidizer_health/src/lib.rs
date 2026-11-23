// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![deny(missing_docs)]

//! Component health reports and HTTP readiness checks for Rust services.
//!
//! A service reports the worst state among its components: `Healthy`, `Degraded`, or `Unhealthy`.
//! For example, if a database is degraded and a cache is healthy, the service is degraded.
//! Use the overall state for readiness checks and [component reports](Report) to find the reason.
//!
//! # Quick start
//!
//! Register a component for the database. Use its [`Publisher`] to report slow queries:
//!
//! ```
//! # testing_aids::init_tracing!();
//! use oxidizer_health::{Aggregator, Health};
//!
//! let aggregator = Aggregator::new();
//! let database = aggregator.component("database");
//! let mut publisher = database.publisher();
//!
//! publisher.degraded("slow queries");
//! assert_eq!(publisher.signal().state(), Health::Degraded);
//! ```
//!
//! Publishing updates the local signal immediately. [`Aggregator::state`] reads the latest
//! aggregate, so it can briefly report the previous state. A component without publishers is
//! `Unhealthy`, as is a service without components.
//!
//! # Write a custom health check
//!
//! Your application decides what to measure, when to measure it, and what counts as unhealthy.
//! A check is ordinary Rust code that publishes its result.
//! For example, a database client can count consecutive failures and reset the count after a
//! successful request. After each request, it passes the current count to this function:
//!
//! ```
//! # testing_aids::init_tracing!();
//! use oxidizer_health::{Aggregator, Health, Publisher};
//!
//! fn report_failures(publisher: &mut Publisher, failures: u32) {
//!     let state = match failures {
//!         0 => Health::Healthy,
//!         1..=2 => Health::Degraded,
//!         _ => Health::Unhealthy,
//!     };
//!     publisher.publish(state, [("consecutive_failures", i64::from(failures))]);
//! }
//!
//! let aggregator = Aggregator::new();
//! let database = aggregator.component("database");
//! let mut publisher = database.publisher();
//!
//! report_failures(&mut publisher, 3);
//! assert_eq!(publisher.signal().state(), Health::Unhealthy);
//!
//! report_failures(&mut publisher, 0);
//! assert_eq!(publisher.signal().state(), Health::Healthy);
//! ```
//!
//! These thresholds are the client's policy. A new publisher starts `Healthy`. Publishing
//! `Unhealthy` immediately afterward does not prevent a probe from seeing that initial `Healthy`.
//!
//! If required dependencies must block readiness at startup, register their components on a new
//! [`Aggregator`] before creating any publishers or serving probes. Keep each component alive and
//! empty until its first successful check. An empty component is `Unhealthy`. After success,
//! create its publisher and reuse it for later failures and recoveries. Dropping the publisher
//! removes its contribution. Dropping the last [`Component`] handle unregisters the component.
//!
//! # Diagnose a change
//!
//! When readiness is false, a read-only [`Monitor`] can identify the component and show what its
//! publishers reported. This observer prints the component summary followed by each distinct problem:
//!
//! ```
//! # testing_aids::init_tracing!();
//! use oxidizer_health::{Filter, Health, Monitor};
//!
//! async fn show_problems(monitor: &Monitor) {
//!     for report in monitor.reports(Filter::DEGRADED | Filter::UNHEALTHY).await {
//!         if report.state() == Health::Healthy {
//!             continue;
//!         }
//!         println!("{report}");
//!         for state in [Health::Degraded, Health::Unhealthy] {
//!             for (signal, _) in report.signals(state) {
//!                 println!("  {signal}");
//!             }
//!         }
//!     }
//! }
//! # futures::executor::block_on(async {
//! #     let aggregator = oxidizer_health::Aggregator::new();
//! #     let database = aggregator.component("database");
//! #     let mut monitor = aggregator.monitor();
//! #     let mut publisher = database.publisher();
//! #     publisher.degraded("slow queries");
//! #     while monitor.state() != Health::Degraded { monitor.changed().await; }
//! #     show_problems(&monitor).await;
//! # });
//! ```
//!
//! For the quick start, the report names `database` and the signal reads `Degraded (reason=slow queries)`.
//! The filter selects signals, not components. A component without publishers has an `Unhealthy`
//! summary but no signal to print. For continuous monitoring, read the reports, await
//! [`Monitor::changed`], then read again. Multiple updates can produce one notification.
//!
//! # HTTP readiness
//!
//! Health reporting works without HTTP. Enable `http` to use `http::service` with Tower,
//! or `request-handler` to use `http::request_handler` with `http_extensions`.
//! Both return HTTP 200 with JSON `true` for `Healthy` and HTTP 503 with JSON `false` otherwise.
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
