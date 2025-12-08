// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::unchecked_duration_subtraction,
        reason = "allow these lints in tests to improve the readability of the tests"
    )
)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Provides primitives to interact with and manipulate machine time.
//!
//! # Quick Start
//!
//! ```no_run
//! use std::time::Duration;
//! use tick::Clock;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Create a clock using the Tokio runtime
//!     let clock = Clock::new_tokio();
//!
//!     // Get the current timestamp
//!     let now = clock.timestamp();
//!     println!("Current time: {now}");
//!
//!     // Delay execution
//!     clock.delay(Duration::from_secs(1)).await;
//!     println!("1 second later: {}", clock.timestamp());
//! }
//! ```
//!
//! # Why?
//!
//! This crate provides a unified API for working with time that:
//!
//! - **Abstracts async runtimes** - Works across Tokio, async-std, etc. without tight coupling
//!   to any specific implementation.
//! - **Enables deterministic testing** - With the `test-util` feature, `ClockControl` lets you
//!   manipulate time flow—advance it instantly, pause it, or jump forward. No waiting for a
//!   1-minute periodic job in your tests.
//! - **Improves testability** - Time-dependent code becomes fast and reproducible to test
//!   without relying on wall-clock time.
//!
//! The testability features are transparent to consumers—code using [`Clock`] works identically
//! in production and tests, with zero runtime overhead when `test-util` is disabled.
//!
//! # Overview
//!
//! - [`Clock`] - Interacts with and controls the flow of time. Provides absolute time
//!   as `SystemTime` or optionally `Timestamp`, and relative time measurements via stopwatch.
//!   Used when creating other time primitives.
//! - [`Timestamp`] - Represents an absolute point in time with formatting, parsing, and
//!   serialization capabilities. Available when the `timestamp` feature is enabled.
//! - [`Stopwatch`] - Measures elapsed time.
//! - [`PeriodicTimer`] - Schedules a task to run periodically.
//! - [`FutureExt`] - Extensions for the `Future` trait.
//! - [`Error`] - Represents an error that can occur when working with time. Introspection is limited.
//! - `ClockControl` - Provides a way to control the flow of time. Exposed only when the `test-util` feature is enabled.
//!
//! # Machine-Centric vs. Human-Centric Time
//!
//! When working with time, two different use cases are considered:
//!
//! - **Machine-Centric** - Measuring time intervals such as timeouts, periodic activities,
//!   cache TTLs, etc. For persistent data, this includes storing, retrieving, and manipulating
//!   timestamps, as well as parsing timestamps in well-known formats such as ISO 8601.
//!   Machine-centric time has little ambiguity.
//! - **Human-Centric** - Wall clock time, formatting, parsing, time zones, calendars.
//!   Dealing with human-centric time involves significant ambiguity.
//!
//! This crate is designed for machine-centric time. For human-centric time manipulation,
//! consider using other crates such as [jiff], [chrono], or [time]. The time primitives in
//! this crate are designed for easy interoperability with these crates. See the `time_interop*`
//! examples for more details.
//!
//! [jiff]: https://crates.io/crates/jiff
//! [chrono]: https://crates.io/crates/chrono
//! [time]: https://crates.io/crates/time
//!
//! # Testing
//!
//! This crate provides a way to control the flow of time in tests via the `ClockControl`
//! type, which is exposed when the `test-util` feature is enabled.
//!
//! **Important:** Never enable the `test-util` feature for production code. Only use it in your `dev-dependencies`.
//!
//! # Examples
//!
//! ## Use `Clock` to retrieve absolute time
//!
//! The clock provides absolute time in two forms. See [`Clock`] documentation for detailed
//! information on when to use each type.
//!
//! ```
//! use std::time::{Duration, SystemTime};
//!
//! use tick::Clock;
//!
//! # fn retrieve_absolute_time(clock: &Clock) {
//! // Using SystemTime for basic absolute time needs
//! let time1: SystemTime = clock.system_time();
//! let time2: SystemTime = clock.system_time();
//!
//! // Time is always moving forward. Note that system time might be
//! // adjusted by the operating system between calls.
//! assert!(time1 <= time2);
//! # }
//! ```
//!
//! With the `timestamp` feature enabled, you can use `Timestamp` for enhanced capabilities:
//!
//! ```
//! use std::time::Duration;
//!
//! use tick::Clock;
//!
//! # fn retrieve_timestamp(clock: &Clock) {
//! // Using Timestamp for formatting, serialization, and cross-process scenarios
//! let timestamp1 = clock.timestamp();
//! let timestamp2 = clock.timestamp();
//!
//! assert!(timestamp1 <= timestamp2);
//! # }
//! ```
//!
//! ## Use `Stopwatch` for measurements
//!
//! ```
//! use std::time::Duration;
//!
//! use tick::{Clock, Stopwatch};
//!
//! # fn measure(clock: &Clock) -> Duration {
//! let stopwatch = Stopwatch::new(clock);
//! // Perform some operations...
//! stopwatch.elapsed()
//! # }
//! ```
//!
//! ## Use `Clock` to create a `PeriodicTimer`
//!
//! ```
//! use std::time::Duration;
//!
//! use futures::StreamExt;
//! use tick::{Clock, Delay, PeriodicTimer, Stopwatch};
//!
//! # async fn periodic_timer_example(clock: &Clock) {
//! // Delay for 10ms before the timer starts ticking
//! Delay::new(clock, Duration::from_millis(10)).await;
//!
//! let timer = PeriodicTimer::new(clock, Duration::from_millis(1));
//!
//! timer
//!     .take(3)
//!     .for_each(async |()| {
//!         // Do something every 1ms
//!     })
//!     .await;
//! # }
//! ```
//!
//! # Additional Examples
//!
//! The [time examples](https://github.com/microsoft/oxidizer/tree/main/crates/tick/examples)
//! contain additional examples of how to use the time primitives.

mod clock;
#[cfg(any(feature = "test-util", test))]
mod clock_control;
mod delay;
mod error;

#[cfg(any(feature = "timestamp", test))]
#[cfg_attr(docsrs, doc(cfg(feature = "timestamp")))]
pub mod fmt;

mod future_ext;
mod periodic_timer;
mod state;
mod stopwatch;
mod timers;

#[cfg(any(feature = "test-util", test))]
mod clock_timestamp;
pub mod runtime;
mod timeout;
#[cfg(any(feature = "timestamp", test))]
mod timestamp;

pub use clock::Clock;
#[cfg(any(feature = "test-util", test))]
#[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
pub use clock_control::ClockControl;
#[cfg(any(feature = "test-util", test))]
#[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
pub use clock_timestamp::ClockTimestamp;
pub use delay::Delay;
pub use error::{Error, Result};
pub use future_ext::FutureExt;
pub use periodic_timer::PeriodicTimer;
pub use stopwatch::Stopwatch;
pub use timeout::Timeout;
#[allow(
    unused_imports,
    reason = "The Timers symbol is flagged as unused when building docs, not sure why..."
)]
pub(crate) use timers::{TIMER_RESOLUTION, TimerKey, Timers};
#[cfg(any(feature = "timestamp", test))]
#[cfg_attr(docsrs, doc(cfg(feature = "timestamp")))]
#[doc(inline)]
pub use timestamp::Timestamp;
