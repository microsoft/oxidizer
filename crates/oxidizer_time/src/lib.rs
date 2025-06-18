// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::unchecked_duration_subtraction,
        reason = "Allow these lints in tests to improve the readability of the tests"
    )
)]

//! Time module provides primitives to interact and manipulate the machine time.
//!
//! - [`Clock`]. Provides a way to interact and control the flow of time. This includes
//!     both retrieval and manipulation of absolute time - [`Timestamp`] or relative
//!     time - [`Stopwatch`]. Clock is used as an argument when creating other time
//!     primitives such as [`PeriodicTimer`] or [`Stopwatch`].
//! - [`Timestamp`]. Represents an absolute point in time.
//! - [`Stopwatch`]. Provides a way to measure elapsed time.
//! - [`PeriodicTimer`]. Provides a way to schedule a task periodically.
//! - [`FutureExt`]. Oxidizer-specific extensions for the `Future` trait.
//! - [`Error`]. Represents an error that can occur when working with time. Introspection is limited.
#![cfg_attr(
    feature = "fakes",
    doc = r"
 - [`ClockControl`]. Provides a way to control the flow of time. Exposed only when the `fakes` feature is enabled.
"
)]
#![cfg_attr(
    not(feature = "fakes"),
    doc = r"
 - `ClockControl`. Provides a way to control the flow of time. Exposed only when the `fakes` feature is enabled.
"
)]
//!
//! # Machine Centric vs Human Centric Time
//!
//! When working with time, two different uses are considered:
//!
//! - **Machine Centric**. This is about measuring some time intervals such as timeouts,
//!     periodic activities, cache TTLs, etc. For persistent data, we also need to
//!     allow storing and subsequent retrieval and manipulation of timestamps. This also
//!     includes parsing of timestamps in well-known formats such as ISO 8601. Machine
//!     centric time has little ambiguity to it.
//! - **Human Centric**. Wall clock time, formatting, parsing, time zones, calendars.
//!     Dealing with human centric time involves a lot of ambiguity.
//!
//! The `time` module is designed to work with machine centric time. If you want to manipulate human centric time,
//! consider using other crates such as [jiff], [chrono] or [time]. The time primitives in this module are designed
//! for easy interoperability with these crates. See the `time_interop*` examples for more details.
//!
//! [jiff]: https://crates.io/crates/jiff
//! [chrono]: https://crates.io/crates/chrono
//! [time]: https://crates.io/crates/time
//!
//! # Testing
//!
//! The time module provides a way to control the flow of time in tests. This is done by using the `ClockControl` type that
//! exposed when the `fakes` feature is enabled for the `oxidizer` crate.
//!
//! Note that you should never enable the `fakes` feature for production code. Only ever use it in your `dev-dependencies`.
#![cfg_attr(
    feature = "fakes",
    doc = r"

 For more information, visit the documentation for [`ClockControl`] type.
"
)]
//!
//! # Examples
//!
//! ### Use `Clock` to get the current `Timestamp`
//!
//! ```
//! use std::time::Duration;
//! use oxidizer_time::Clock;
//!
//! fn retrieve_timestamp(clock: &Clock) {
//!     let timestamp1 = clock.now();
//!     let timestamp2 = clock.now();
//!
//!     // Time is always moving forward. Note that system time might be
//!     // adjusted by operating system between the "now" calls.
//!     assert!(timestamp1 <= timestamp2);
//! }
//! # let clock = Clock::with_control(&oxidizer_time::ClockControl::new().auto_advance(Duration::from_secs(1)));
//! # retrieve_timestamp(&clock);
//! ```
//!
//! ### Use `Stopwatch` for measurements
//!
//! ```
//! use std::time::Duration;
//! use oxidizer_time::{Clock, Stopwatch};
//!
//! fn measure(clock: &Clock) -> Duration {
//!     let stopwatch = Stopwatch::with_clock(clock);
//!     // Perform some operations ...
//!     stopwatch.elapsed()
//! }
//! # let clock = Clock::with_control(&oxidizer_time::ClockControl::new());
//! # measure(&clock);
//! ```
//!
//! ### Use `Clock` to create a `PeriodicTimer`
//!
//! ```
//! use oxidizer_time::{Clock, Stopwatch, PeriodicTimer, Delay};
//! use std::time::Duration;
//! use futures::StreamExt;
//!
//! async fn periodic_timer_example(clock: &Clock) {
//!     // Delay for 10ms before timer starts ticking
//!     Delay::with_clock(clock, Duration::from_millis(10)).await;
//!
//!     let timer = PeriodicTimer::with_clock(clock, Duration::from_millis(1));
//!
//!     timer
//!         .take(3)
//!         .for_each(async |()| {
//!             // Do something every 1ms
//!         })
//!         .await;
//! }
//!
//! # fn main() {
//! #     let control = oxidizer_time::ClockControl::new().auto_advance_timers(true);
//! #     let clock = Clock::with_control(&control);
//! #     futures::executor::block_on(periodic_timer_example(&clock));
//! # }
//! ```
//!
//! Additional examples
//!
//! The [time examples](https://github.com/microsoft/oxidizer/blob/main/crates/oxidizer_time/examples)
//! contains additional examples of how to use the time primitives.

mod clock;
#[cfg(any(feature = "fakes", test))]
mod clock_control;
mod delay;
mod error;

pub mod fmt;
mod future_ext;
mod periodic_timer;
mod state;
mod stopwatch;
mod timers;
mod timestamp;

mod duration_ext;
pub mod runtime;
mod timeout;

pub use clock::*;
#[cfg(any(feature = "fakes", test))]
pub use clock_control::*;
pub use delay::*;
pub use duration_ext::*;
pub use error::*;
pub use future_ext::*;
// No need to re-define the timeout future API, it's really minimal and already documented.
pub use periodic_timer::*;
pub use stopwatch::*;
pub use timeout::*;
#[allow(
    unused_imports,
    reason = "The Timers symbol is flagged as unused when building docs, not sure why..."
)]
pub(crate) use timers::{TIMER_RESOLUTION, TimerKey, Timers};
pub use timestamp::*;