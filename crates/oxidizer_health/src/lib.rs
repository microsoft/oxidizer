// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![deny(missing_docs)]

//! Application health aggregation.
//!
//! Application code owns [`Publisher`]s beside the workers or clients that detect health
//! changes. Each publisher publishes a [`Signal`] containing a [`Health`] state and
//! [`Attribute`]s. A [`Component`] aggregates the publishers of one subsystem, and the
//! [`Aggregator`] exposes process health and component [`Report`]s.
//!
//! Aggregation is worst-state-wins at both levels. A component without publishers and an
//! aggregator without components are `Unhealthy`.
//!
//! [`state`](Aggregator::state) is a cheap snapshot read for readiness decisions.
//! [`changed`](Aggregator::changed) is an edge notification: it resolves after the state or report
//! data may have changed, after which consumers read the current state. Publishing never waits for
//! a consumer.
//! If the aggregation worker cannot run, state reads return `Unhealthy` and the failure is
//! reported through diagnostics.
//!
//! # Examples
//!
//! ```
//! # testing_aids::init_tracing!();
//! use oxidizer_health::{Aggregator, Attribute, Filter, Health};
//!
//! # futures::executor::block_on(async {
//! let mut aggregator = Aggregator::new();
//!
//! let startup = aggregator.component("startup");
//! let database = aggregator.component("database");
//!
//! let mut startup_publisher = startup.publisher();
//! let mut primary_publisher = database.publisher();
//! let mut replica_publisher = database.publisher();
//!
//! startup_publisher.publish(Health::Unhealthy, [("phase", "starting")]);
//! primary_publisher.degraded("High latency detected");
//! replica_publisher.publish(Health::Healthy, Vec::<Attribute>::new());
//!
//! startup_publisher.publish(Health::Healthy, Vec::<Attribute>::new());
//! while aggregator.state() != Health::Degraded {
//!     aggregator.changed().await;
//! }
//!
//! for report in aggregator
//!     .reports(Filter::DEGRADED | Filter::UNHEALTHY)
//!     .await
//! {
//!     for (signal, publishers) in report.signals(Health::Degraded) {
//!         println!("{report}: {signal} from {publishers} publishers");
//!     }
//! }
//! # });
//! ```
//!
//! # Features
//!
//! - `http`: `http::service`, a Tower readiness service.
//! - `request-handler`: `http::request_handler`, an `http_extensions` readiness request
//!   handler.

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
