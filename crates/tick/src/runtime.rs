// Copyright (c) Microsoft Corporation.

//! Infrastructure for integrating time primitives into async runtimes.
//!
//! This module provides the necessary components to bridge time-based operations
//! with async runtime execution. The primary workflow involves:
//!
//! 1. Start with an [`InactiveClock`] that can be safely moved across threads
//! 2. Activate it using [`InactiveClock::activate`] to get a [`Clock`] and [`ClockDriver`]
//! 3. Use the [`ClockDriver`] to periodically advance timers in your runtime loop
//! 4. Use the [`Clock`] for time operations like creating timers and measuring time
//!
//! # Integration with Runtimes
//!
//! Different runtime architectures can integrate this module as follows:
//!
//! ## Single-threaded Runtimes
//!
//! Clone the [`InactiveClock`] for each thread and activate separately to avoid
//! lock contention:
//!
//! ```rust
//! # use tick::runtime::InactiveClock;
//! let inactive = InactiveClock::default();
//! let inactive_clone = inactive.clone();
//!
//! // On thread 1
//! let (clock1, driver1) = inactive.activate();
//!
//! // On thread 2
//! let (clock2, driver2) = inactive_clone.activate();
//! ```
//!
//! ## Multi-threaded Runtimes
//!
//! Activate once and share the clock across threads, while keeping the driver
//! on the main runtime thread for timer advancement.
//!
//! [`Clock`]: crate::Clock
//! [`InactiveClock::activate`]: InactiveClock::activate

mod clock_driver;
mod inactive_clock;

pub use clock_driver::ClockDriver;
pub use inactive_clock::InactiveClock;
