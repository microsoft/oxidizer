// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/tick/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/tick/favicon.ico")]
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::unchecked_time_subtraction,
        reason = "allow these lints in tests to improve the readability of the tests"
    )
)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Primitives for obtaining, working with, and mocking system
//! time and timers, enabling faster and more robust testing.
//!
//! # Quick Start
//!
//! ```no_run
//! use std::time::Duration;
//! use tick::{Clock, Delay};
//!
//! async fn produce_value(clock: &Clock) -> u64 {
//!     let stopwatch = clock.stopwatch();
//!     clock.delay(Duration::from_secs(60)).await;
//!     println!("elapsed time: {}ms", stopwatch.elapsed().as_millis());
//!     123
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     let clock = Clock::new_tokio();
//!     let value = produce_value(&clock).await;
//!     assert_eq!(value, 123);
//! }
//!
//! #[cfg(test)]
//! mod tests {
//!     use super::*;
//!     use tick::ClockControl;
//!
//!     #[tokio::test]
//!     async fn test_produce_value() {
//!         // Automatically advance timers for instant, deterministic testing
//!         let clock: Clock = ClockControl::new().auto_advance_timers(true).to_clock();
//!         assert_eq!(produce_value(&clock).await, 123);
//!     }
//! }
//! ```
//!
//! # Why?
//!
//! This crate provides a unified API for working with time that:
//!
//! - **Easy async runtime integration** - Provides built-in support for Tokio and can be extended
//!   to work with other runtimes without tight coupling to any specific implementation.
//! - **Enables deterministic testing** - With the `test-util` feature, [`ClockControl`] lets you
//!   manipulate the passage of time—advance it instantly, pause it, or jump forward. No waiting
//!   for a 1-minute periodic job in your tests.
//! - **Improves testability** - Time-dependent code becomes fast and reproducible to test
//!   without relying on wall-clock time.
//!
//! The testability features are transparent to consumers—code using [`Clock`] works identically
//! in production and tests, with zero runtime overhead when `test-util` is disabled.
//!
//! # Overview
//!
//! - [`Clock`] - Provides an abstraction for time-related operations. Returns absolute time
//!   as `SystemTime` and relative time measurements via stopwatch. Used when creating other
//!   time primitives.
//! - [`ClockControl`] - Controls the passage of time. Available when the `test-util` feature
//!   is enabled.
//! - [`Stopwatch`] - Measures elapsed time.
//! - [`Delay`] - Delays the execution for a specified duration.
//! - [`PeriodicTimer`] - Schedules a task to run periodically.
//! - [`FutureExt`] - Extensions for the `Future` trait.
//! - [`Error`] - Represents an error that can occur when working with time. Provides limited
//!   introspection capabilities.
//! - [`fmt`] - Utilities for formatting `SystemTime` into various formats. Available when
//!   the `fmt` feature is enabled.
//! - [`runtime`] - Infrastructure for integrating time primitives into async runtimes.
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
//! This crate provides a way to control the passage of time in tests via the `ClockControl`
//! type, which is exposed when the `test-util` feature is enabled.
//!
//! > **Important:** Never enable the `test-util` feature for production code. Only use it in your `dev-dependencies`.
//!
//! # Examples
//!
//! ## Use `Clock` to retrieve absolute time
//!
//! The clock provides absolute time as `SystemTime`. See [`Clock`] documentation for detailed
//! information.
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
//! ## Use `Clock` to retrieve relative time
//!
//! The clock provides relative time via [`Clock::instant`] and [`Stopwatch`].
//!
//! ```
//! use std::time::{Duration, Instant};
//!
//! use tick::Clock;
//!
//! # fn retrieve_relative_time(clock: &Clock) {
//! // Using clock.stopwatch() for convenient elapsed time measurement
//! let stopwatch = clock.stopwatch();
//! // Perform some operation...
//! let elapsed: Duration = stopwatch.elapsed();
//!
//! // Using Clock::instant for lower-level access to monotonic time
//! let start: Instant = clock.instant();
//! // Perform some operation...
//! let end: Instant = clock.instant();
//! # }
//! ```
//!
//! ## Use `Stopwatch` for measurements
//!
//! ```
//! use std::time::Duration;
//!
//! use tick::Clock;
//!
//! # fn measure(clock: &Clock) -> Duration {
//! let stopwatch = clock.stopwatch();
//! // Perform some operation...
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
//! use tick::{Clock, PeriodicTimer};
//!
//! # async fn periodic_timer_example(clock: &Clock) {
//! // Delay for 10ms before the timer starts ticking
//! clock.delay(Duration::from_millis(10)).await;
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
//! # Features
//!
//! This crate provides several optional features that can be enabled in your `Cargo.toml`:
//!
//! - **`tokio`** - Integration with the [Tokio](https://tokio.rs/) runtime. Enables
//!   [`Clock::new_tokio`] for creating clocks that use Tokio's time facilities.
//! - **`test-util`** - Enables the [`ClockControl`] type for controlling the passage of time
//!   in tests. This allows you to pause time, advance it manually, or automatically advance
//!   timers for fast, deterministic testing. **Only enable this in `dev-dependencies`.**
//! - **`serde`** - Adds serialization and deserialization support via [serde](https://serde.rs/).
//! - **`fmt`** - Enables the [`fmt`] module with utilities for formatting `SystemTime` into
//!   various formats (e.g., ISO 8601, RFC 2822).
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

#[cfg(any(feature = "fmt", test))]
#[cfg_attr(docsrs, doc(cfg(feature = "fmt")))]
pub mod fmt;

mod future_ext;
mod periodic_timer;
mod state;
mod stopwatch;
mod timers;

pub mod runtime;
pub(crate) mod timeout;
pub use clock::Clock;
#[cfg(any(feature = "test-util", test))]
#[cfg_attr(docsrs, doc(cfg(feature = "test-util")))]
pub use clock_control::ClockControl;
pub use delay::Delay;
pub use error::{Error, Result};
pub use future_ext::FutureExt;
pub use periodic_timer::PeriodicTimer;
pub use stopwatch::Stopwatch;
pub use timeout::Timeout;
