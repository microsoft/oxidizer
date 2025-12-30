<div align="center">
 <img src="./logo.png" alt="Tick Logo" width="96">

# Tick

[![crate.io](https://img.shields.io/crates/v/tick.svg)](https://crates.io/crates/tick)
[![docs.rs](https://docs.rs/tick/badge.svg)](https://docs.rs/tick)
[![MSRV](https://img.shields.io/crates/msrv/tick)](https://crates.io/crates/tick)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Primitives for obtaining, working with, and mocking system
time and timers, enabling faster and more robust testing.

## Quick Start

```rust
use std::time::Duration;
use tick::{Clock, Delay};

async fn produce_value(clock: &Clock) -> u64 {
    let stopwatch = clock.stopwatch();
    clock.delay(Duration::from_secs(60)).await;
    println!("elapsed time: {}ms", stopwatch.elapsed().as_millis());
    123
}

#[tokio::main]
async fn main() {
    let clock = Clock::new_tokio();
    let value = produce_value(&clock).await;
    assert_eq!(value, 123);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tick::ClockControl;

    #[tokio::test]
    async fn test_produce_value() {
        // Automatically advance timers for instant, deterministic testing
        let clock: Clock = ClockControl::new().auto_advance_timers(true).to_clock();
        assert_eq!(produce_value(&clock).await, 123);
    }
}
```

## Why?

This crate provides a unified API for working with time that:

* **Easy async runtime integration** - Provides built-in support for Tokio and can be extended
  to work with other runtimes without tight coupling to any specific implementation.
* **Enables deterministic testing** - With the `test-util` feature, [`ClockControl`][__link0] lets you
  manipulate the passage of time: advance it instantly, pause it, or jump forward. No waiting
  for a 1-minute periodic job in your tests.
* **Improves testability** - Time-dependent code becomes fast and reproducible to test
  without relying on wall-clock time.

The testability features are transparent to consuming code, as using [`Clock`][__link1] works identically
in production and tests, with zero runtime overhead when `test-util` is disabled.

## Overview

* [`Clock`][__link2] - Provides an abstraction for time-related operations. Returns absolute time
  as `SystemTime` and relative time measurements via stopwatch. Used when creating other
  time primitives.
* [`ClockControl`][__link3] - Controls the passage of time. Available when the `test-util` feature
  is enabled.
* [`Stopwatch`][__link4] - Measures elapsed time.
* [`Delay`][__link5] - Delays the execution for a specified duration.
* [`PeriodicTimer`][__link6] - Schedules a task to run periodically.
* [`Error`][__link7] - Represents an error that can occur when working with time. Provides limited
  introspection capabilities.
* [`fmt`][__link8] - Utilities for formatting `SystemTime` into various formats. Available when
  the `fmt` feature is enabled.
* [`runtime`][__link9] - Infrastructure for integrating time primitives into async runtimes.

## Extensions

* [`FutureExt`][__link10] - Extensions for the `Future` trait, providing timeout functionality.
* [`SystemTimeExt`][__link11] - Extensions for [`SystemTime`][__link12].

## Machine-Centric vs. Human-Centric Time

When working with time, two different use cases are considered:

* **Machine-Centric** - Measuring time intervals such as timeouts, periodic activities,
  cache TTLs, etc. For persistent data, this includes storing, retrieving, and manipulating
  timestamps, as well as parsing timestamps in well-known formats such as ISO 8601.
  Machine-centric time has little ambiguity.
* **Human-Centric** - Wall clock time, formatting, parsing, time zones, calendars.
  Dealing with human-centric time involves significant ambiguity.

This crate is designed for machine-centric time. For human-centric time manipulation,
consider using other crates such as [jiff][__link13], [chrono][__link14], or [time][__link15]. The time primitives in
this crate are designed for easy interoperability with these crates. See the `time_interop*`
examples for more details.

## Testing

This crate provides a way to control the passage of time in tests via the `ClockControl`
type, which is exposed when the `test-util` feature is enabled.

 > 
 > **Important**: Never enable the `test-util` feature for production code. Only use it in your `dev-dependencies`.

## Examples

### Use `Clock` to retrieve absolute time

The clock provides absolute time as `SystemTime`. See [`Clock`][__link16] documentation for detailed
information.

```rust
use std::time::{Duration, SystemTime};

use tick::Clock;

// Using SystemTime for basic absolute time needs
let time1: SystemTime = clock.system_time();
let time2: SystemTime = clock.system_time();

// Time is always moving forward. Note that system time might be
// adjusted by the operating system between calls.
assert!(time1 <= time2);
```

### Use `Clock` to retrieve relative time

The clock provides relative time via [`Clock::instant`][__link17] and [`Stopwatch`][__link18].

```rust
use std::time::{Duration, Instant};

use tick::Clock;

// Using clock.stopwatch() for convenient elapsed time measurement
let stopwatch = clock.stopwatch();
// Perform some operation...
let elapsed: Duration = stopwatch.elapsed();

// Using Clock::instant for lower-level access to monotonic time
let start: Instant = clock.instant();
// Perform some operation...
let end: Instant = clock.instant();
```

### Use `Stopwatch` for measurements

```rust
use std::time::Duration;

use tick::Clock;

let stopwatch = clock.stopwatch();
// Perform some operation...
stopwatch.elapsed()
```

### Use `Clock` to create a `PeriodicTimer`

```rust
use std::time::Duration;

use futures::StreamExt;
use tick::{Clock, PeriodicTimer};

// Delay for 10ms before the timer starts ticking
clock.delay(Duration::from_millis(10)).await;

let timer = PeriodicTimer::new(clock, Duration::from_millis(1));

timer
    .take(3)
    .for_each(async |()| {
        // Do something every 1ms
    })
    .await;
```

## Features

This crate provides several optional features that can be enabled in your `Cargo.toml`:

* **`tokio`** - Integration with the [Tokio][__link19] runtime. Enables
  [`Clock::new_tokio`][__link20] for creating clocks that use Tokio’s time facilities.
* **`test-util`** - Enables the [`ClockControl`][__link21] type for controlling the passage of time
  in tests. This allows you to pause time, advance it manually, or automatically advance
  timers for fast, deterministic testing. **Only enable this in `dev-dependencies`.**
* **`serde`** - Adds serialization and deserialization support via [serde][__link22].
* **`fmt`** - Enables the [`fmt`][__link23] module with utilities for formatting `SystemTime` into
  various formats (e.g., ISO 8601, RFC 2822).

## Additional Examples

The [time examples][__link24]
contain additional examples of how to use the time primitives.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/tick">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG7RNcoP3blXnG4CHqkinKPcBG0KvcQDgsaxXGxhnbuGo0h6JYWSBgmR0aWNrZTAuMS4w
 [__link0]: https://docs.rs/tick/0.1.0/tick/?search=ClockControl
 [__link1]: https://docs.rs/tick/0.1.0/tick/?search=Clock
 [__link10]: https://docs.rs/tick/0.1.0/tick/?search=FutureExt
 [__link11]: https://docs.rs/tick/0.1.0/tick/?search=SystemTimeExt
 [__link12]: https://doc.rust-lang.org/stable/std/?search=time::SystemTime
 [__link13]: https://crates.io/crates/jiff
 [__link14]: https://crates.io/crates/chrono
 [__link15]: https://crates.io/crates/time
 [__link16]: https://docs.rs/tick/0.1.0/tick/?search=Clock
 [__link17]: https://docs.rs/tick/0.1.0/tick/?search=Clock::instant
 [__link18]: https://docs.rs/tick/0.1.0/tick/?search=Stopwatch
 [__link19]: https://tokio.rs/
 [__link2]: https://docs.rs/tick/0.1.0/tick/?search=Clock
 [__link20]: https://docs.rs/tick/0.1.0/tick/?search=Clock::new_tokio
 [__link21]: https://docs.rs/tick/0.1.0/tick/?search=ClockControl
 [__link22]: https://serde.rs/
 [__link23]: https://docs.rs/tick/0.1.0/tick/fmt/index.html
 [__link24]: https://github.com/microsoft/oxidizer/tree/main/crates/tick/examples
 [__link3]: https://docs.rs/tick/0.1.0/tick/?search=ClockControl
 [__link4]: https://docs.rs/tick/0.1.0/tick/?search=Stopwatch
 [__link5]: https://docs.rs/tick/0.1.0/tick/?search=Delay
 [__link6]: https://docs.rs/tick/0.1.0/tick/?search=PeriodicTimer
 [__link7]: https://docs.rs/tick/0.1.0/tick/?search=Error
 [__link8]: https://docs.rs/tick/0.1.0/tick/fmt/index.html
 [__link9]: https://docs.rs/tick/0.1.0/tick/runtime/index.html
