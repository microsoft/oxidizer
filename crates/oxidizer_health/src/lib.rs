// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![deny(missing_docs)]

//! Health signal collection and aggregation for Rust services.
//!
//! [`Publisher`]s collect application-defined health signals into named [`Component`]s.
//! Each component aggregates its publishers, and an [`Aggregator`] combines the components into
//! an overall service state: [`Healthy`](Health::Healthy), [`Degraded`](Health::Degraded), or
//! [`Unhealthy`](Health::Unhealthy). At each level, the worst state wins.
//! [Reports](Report) retain diagnostic attributes and publisher counts.
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
//! Publishing updates the local [`Signal`] immediately. Aggregation runs in the background.
//! Several updates can produce one [`Monitor::changed`] notification. Read the latest state
//! or reports after each notification rather than counting transitions.
//!
//! The loop waits for `Degraded`. If the worker stops or fails to start, readiness is `Unhealthy`.
//! A monitor created after that receives no notification.
//! If a wait must end even when the target state is unreachable, use a timeout or cancellation.
//!
//! Dropping a publisher removes its contribution. A component unregisters after its last
//! [`Component`] handle and publisher drop. A component without publishers is `Unhealthy`, as is
//! an [`Aggregator`] without components.
//!
//! # Diagnose a change
//!
//! A read-only [`Monitor`] provides component reports independently of publishers.
//! This example publishes a problem, waits for aggregation, and prints its diagnostic signal:
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
//! publisher.degraded("connection pool exhausted");
//! while monitor.state() != Health::Degraded {
//!     monitor.changed().await;
//! }
//!
//! for report in monitor.reports().await {
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
//! Reports group identical signals and count their publishers. Call [`Report::signals`] for the
//! severity that the diagnostic consumer needs.
//! An empty component has an `Unhealthy` summary with no signals. For continuous observation,
//! read the reports, await [`Monitor::changed`], and repeat.
//!
//! # Additional documentation
//!
//! - Register a [`Component`] with [`Aggregator::component`] and report changes with [`Publisher::publish`].
//! - Get a read-only [`Monitor`] with [`Aggregator::monitor`] to observe changes independently of publishers.
//! - Select diagnostic signals by [`Health`] and inspect their attributes and publisher counts with [`Report::signals`].

mod aggregator;
mod attribute;
mod component;
mod health;
mod report;
mod signal;
mod worker;

pub use aggregator::{Aggregator, Monitor};
pub use attribute::{Attribute, AttributeValue};
pub use component::{Component, Publisher};
pub use health::Health;
pub use report::Report;
pub use signal::Signal;

#[cfg(test)]
testing_aids::init_tracing!();
