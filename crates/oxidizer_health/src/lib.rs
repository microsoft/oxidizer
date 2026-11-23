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
//! let health = Aggregator::new();
//! let database = health.component("database");
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
//! # HTTP readiness
//!
//! Health reporting works without HTTP. Enable `http` to use `http::service` with Tower,
//! or `request-handler` to use `http::request_handler` with `http_extensions`.
//! Both return HTTP 200 with JSON `true` for `Healthy` and HTTP 503 with JSON `false` otherwise.

mod aggregator;
mod attribute;
mod component;
mod drain;
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
