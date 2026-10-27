// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    not(all(feature = "test-util", feature = "tokio", feature = "fmt")),
    expect(rustdoc::broken_intra_doc_links, reason = "simpler docs")
)]
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

//! Time and timer types for production code and tests.
//!
//! # Quick Start
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use tick::{Clock, Delay};
//!
//! async fn produce_value(clock: &Clock) -> u64 {
//!     let stopwatch = clock.stopwatch();
//!     clock.delay(Duration::from_secs(60)).await;
//!     println!("elapsed time: {}ms", stopwatch.elapsed().as_millis());
//!     123
//! }
//!
//! # #[cfg(feature = "tokio")]
//! #[tokio::main]
//! async fn main() {
//!     let clock = Clock::new_tokio();
//!     let value = produce_value(&clock).await;
//!     assert_eq!(value, 123);
//! }
//! # #[cfg(not(feature = "tokio"))]
//! # fn main() {}
//!
//! #[cfg(test)]
//! mod tests {
//!     use tick::ClockControl;
//!
//!     use super::*;
//!
//!     #[tokio::test]
//!     async fn test_produce_value() {
//!         // Advance timers automatically without a real wait.
//!         let clock: Clock = ClockControl::new().auto_advance_timers(true).to_clock();
//!         assert_eq!(produce_value(&clock).await, 123);
//!     }
//! }
//! ```
//!
//! # Purpose
//!
//! This crate provides one API for production and controlled test time:
//!
//! - The `tokio` feature provides Tokio integration. The [`runtime`] module
//!   supports other async runtimes.
//! - The `test-util` feature provides [`ClockControl`]. Tests can freeze or
//!   advance time without a real wait.
//! - Production and test code use the same [`Clock`] API.
//!
//! If `test-util` is disabled, its support adds no runtime overhead.
//!
//! # Overview
//!
//! - [`Clock`] provides time, delays, and timers.
//! - [`SimpleClock`] reads time without a runtime or timer driver. All clock
//!   types share this interface through [`AsRef<SimpleClock>`].
//! - [`ClockControl`] controls test time. Enable `test-util` to use it.
//! - [`Stopwatch`] measures elapsed time.
//! - [`Delay`] completes after a duration.
//! - [`Timer`] produces periodic ticks.
//! - [`Error`] reports failures and provides limited error inspection.
//! - [`fmt`] parses and displays `SystemTime`. Enable `fmt` to use it.
//! - [`runtime`] connects clocks to async runtimes.
//!
//! The [design guide](https://github.com/microsoft/oxidizer/blob/main/crates/tick/docs/design.md)
//! explains the stable contracts and the internal timer architecture.
//!
//! # Extensions
//!
//! - [`FutureExt`] adds timeouts to `Future`.
//! - [`SystemTimeExt`] adds display formats to
//!   [`SystemTime`][`std::time::SystemTime`].
//!
//! # Time retrieval without timers
//!
//! Use [`SimpleClock`] when an API only reads time. It has no timers, so it
//! needs no async runtime or driver. [`SimpleClock::new_system`] creates a
//! ready-to-use clock that reads operating system time.
//!
//! Each clock type provides the `SimpleClock` interface:
//!
//! - [`Clock`] implements [`AsRef<SimpleClock>`] and exposes
//!   [`simple_clock()`][Clock::simple_clock]. You can pass a timer-capable clock
//!   to an API that accepts `SimpleClock`.
//! - With the `test-util` feature, [`ClockControl::to_simple_clock`] creates a controlled
//!   `SimpleClock`. A [`Clock`] from the same control observes the same time.
//!
//! A time-only API can accept either clock. For example, [`Stopwatch`] accepts
//! any [`AsRef<SimpleClock>`]:
//!
//! ```
//! use tick::{SimpleClock, Stopwatch};
//!
//! // A driver-free clock that only retrieves time.
//! let clock = SimpleClock::new_system();
//!
//! let _now = clock.system_time();
//!
//! // `Stopwatch` accepts a `SimpleClock` or a `Clock` (both are `AsRef<SimpleClock>`).
//! let stopwatch = Stopwatch::new(&clock);
//! let _elapsed = stopwatch.elapsed();
//! ```
//!
//! # Machine time and human time
//!
//! Time has two common uses:
//!
//! - Machine time measures intervals, timeouts, periodic work, and cache
//!   lifetimes. It also stores and parses timestamps.
//! - Human time uses calendars, local clock time, and time zones. These values
//!   can be ambiguous.
//!
//! This crate focuses on machine time. Use [jiff], [chrono], or [time] for
//! human time. `tick` values can convert to types from these crates. See the
//! `interop_*` examples.
//!
//! [jiff]: https://crates.io/crates/jiff
//! [chrono]: https://crates.io/crates/chrono
//! [time]: https://crates.io/crates/time
//!
//! # Thread-aware relocation
//!
//! All clock types implement [`ThreadAware`](thread_aware::ThreadAware).
//! Thread-isolated runtimes can use it to isolate timers.
//!
//! If an [`InactiveClock`][runtime::InactiveClock] moves between coordinates
//! with the same runtime owner, each destination thread gets separate timer
//! storage. After activation, each thread has an independent [`Clock`] and
//! [`ClockDriver`][runtime::ClockDriver]. A move to another owner keeps the
//! existing storage instead.
//!
//! Relocation does not change a [`ClockControl`] clock. Its clones share one
//! controlled time state across threads.
//!
//! See the [`runtime`] module documentation for setup examples.
//!
//! # Testing
//!
//! Enable `test-util` to use [`ClockControl`] in tests.
//!
//! Do not enable `test-util` in production dependencies. Enable it only in
//! `dev-dependencies`. The feature adds test support that reduces production
//! performance.
//!
//! # Examples
//!
//! ## Use `Clock` to retrieve absolute time
//!
//! A clock returns absolute time as `SystemTime`. See [`Clock`] for details.
//!
//! ```
//! use std::time::{Duration, SystemTime};
//!
//! use tick::Clock;
//!
//! # fn retrieve_absolute_time(clock: &Clock) {
//! // Read two absolute times.
//! let time1: SystemTime = clock.system_time();
//! let time2: SystemTime = clock.system_time();
//!
//! // The operating system can adjust system time between these calls.
//! assert!(time1 <= time2);
//! # }
//! ```
//!
//! ## Use `Clock` to retrieve relative time
//!
//! A clock provides relative time through [`Clock::instant`] and [`Stopwatch`].
//!
//! ```
//! use std::time::{Duration, Instant};
//!
//! use tick::Clock;
//!
//! # fn retrieve_relative_time(clock: &Clock) {
//! // Use a stopwatch to measure elapsed time.
//! let stopwatch = clock.stopwatch();
//! // Perform some operation...
//! let elapsed: Duration = stopwatch.elapsed();
//!
//! // Use `Clock::instant` for direct access to monotonic time.
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
//! ## Use `Clock` to create a `Timer`
//!
//! ```
//! use std::time::Duration;
//!
//! use tick::{Clock, Timer};
//!
//! # async fn periodic_timer_example(clock: &Clock) {
//! // Wait 10 milliseconds before the first timer starts.
//! clock.delay(Duration::from_millis(10)).await;
//!
//! let mut timer = Timer::new(clock, Duration::from_millis(1));
//!
//! for _ in 0..3 {
//!     timer.next().await;
//!     // Run periodic work.
//! }
//! # }
//! ```
//!
//! # Features
//!
//! You can enable these features in `Cargo.toml`:
//!
//! - `tokio` adds [Tokio](https://tokio.rs/) integration and
//!   [`Clock::new_tokio`].
//! - `test-util` adds [`ClockControl`] for deterministic tests. It can freeze
//!   time, advance time, or advance timers automatically. Enable it only in
//!   `dev-dependencies`.
//! - `serde` adds serialization through [serde](https://serde.rs/).
//! - `fmt` adds the [`fmt`] module and formats such as ISO 8601 and RFC 2822.
//! - `fast-instant` lets each clock clone select a lower-overhead,
//!   lower-precision instant source on Linux and Windows. Timer scheduling stays
//!   precise. Other platforms use [`std::time::Instant::now`]. Precise reads
//!   remain the default.
//!
//! ## Fast instant contract
//!
//! Fast instant reads are disabled by default. Each clock clone has its own
//! setting. Values do not decrease, but consecutive reads can be equal. The
//! platform defines their precision. A stopwatch keeps the source setting of
//! its clock.
//!
//! This option does not affect `system_time()`, controlled clocks, or precise
//! timer scheduling. On an unsupported platform, it keeps the standard instant
//! source.
//!
//! ## Using precise and fast clocks together
//!
//! A component can create a fast clone from an existing [`Clock`]. It does not
//! need a separate application dependency. The original clock stays precise:
//!
//! ```
//! # fn main() {
//! # #[cfg(feature = "fast-instant")] {
//! use tick::Clock;
//!
//! fn process(clock: &Clock) {
//!     let fast_clock = clock.clone().with_fast_instant(true);
//!
//!     let decision_time = clock.instant();
//!     let telemetry_time = fast_clock.instant();
//!     // Use precise time for logic and fast time for debug data.
//! #   let _ = (decision_time, telemetry_time);
//! }
//! # }
//! # }
//! ```
//!
//! # More examples
//!
//! The [time examples](https://github.com/microsoft/oxidizer/tree/main/crates/tick/examples)
//! show more uses of these types.

mod clock;
#[cfg(any(feature = "test-util", test))]
mod clock_control;
mod delay;
mod error;
#[cfg(any(feature = "fast-instant", test))]
mod fast_instant;

#[cfg(any(feature = "fmt", test))]
pub mod fmt;

mod future_ext;
mod simple_clock;
mod state;
mod stopwatch;
#[cfg(any(feature = "fmt", test))]
mod system_time_ext;
mod timer;
mod timers;
#[cfg(any(feature = "tokio", test))]
mod tokio;

pub mod runtime;
pub(crate) mod timeout;
pub use clock::Clock;
#[cfg(any(feature = "test-util", test))]
pub use clock_control::ClockControl;
pub use delay::Delay;
pub use error::Error;
pub use future_ext::FutureExt;
pub use simple_clock::SimpleClock;
pub use stopwatch::Stopwatch;
#[cfg(any(feature = "fmt", test))]
pub use system_time_ext::SystemTimeExt;
pub use timeout::Timeout;
pub use timer::{Timer, TimerTick};

/// Implements [`ThreadAware`](thread_aware::ThreadAware) for types that need no
/// special relocation work.
macro_rules! thread_aware_move {
    ($($t:ty),+ $(,)?) => {
        $(
            impl thread_aware::ThreadAware for $t {
                fn relocate(&mut self, _source: Option<&thread_aware::Thread>, _destination: &thread_aware::Thread) {}
            }
        )+
    };
}

pub(crate) use thread_aware_move;
