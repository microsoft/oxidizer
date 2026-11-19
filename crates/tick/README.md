<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Tick Logo" width="96">

# Tick

[![crate.io](https://img.shields.io/crates/v/tick.svg)](https://crates.io/crates/tick)
[![docs.rs](https://docs.rs/tick/badge.svg)](https://docs.rs/tick)
[![MSRV](https://img.shields.io/crates/msrv/tick)](https://crates.io/crates/tick)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Time and timer types for production code and tests.

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
    use tick::ClockControl;

    use super::*;

    #[tokio::test]
    async fn test_produce_value() {
        // Advance timers automatically without a real wait.
        let clock: Clock = ClockControl::new().auto_advance_timers(true).to_clock();
        assert_eq!(produce_value(&clock).await, 123);
    }
}
```

## Purpose

This crate provides one API for production and controlled test time:

* The `tokio` feature provides Tokio integration. The [`runtime`][__link0] module
  supports other async runtimes.
* The `test-util` feature provides [`ClockControl`][__link1]. Tests can freeze or
  advance time without a real wait.
* Production and test code use the same [`Clock`][__link2] API.

If `test-util` is disabled, its support adds no runtime overhead.

## Overview

* [`Clock`][__link3] provides time, delays, and timers.
* [`SimpleClock`][__link4] reads time without a runtime or timer driver. All clock
  types share this interface through [`AsRef<SimpleClock>`][__link5].
* [`ClockControl`][__link6] controls test time. Enable `test-util` to use it.
* [`Stopwatch`][__link7] measures elapsed time.
* [`Delay`][__link8] completes after a duration.
* [`Timer`][__link9] produces periodic ticks.
* [`Error`][__link10] reports failures and provides limited error inspection.
* [`runtime`][__link11] connects clocks to async runtimes.

The [design guide][__link12]
explains the runtime-neutral boundary and the internal timer architecture.

## Extensions

* [`FutureExt`][__link13] adds timeouts to `Future`.

## Time retrieval without timers

Use [`SimpleClock`][__link14] when an API only reads time. It has no timers, so it
needs no async runtime or driver. [`SimpleClock::new_system`][__link15] creates a
ready-to-use clock that reads operating system time.

Each clock type provides the `SimpleClock` interface:

* [`Clock`][__link16] implements [`AsRef<SimpleClock>`][__link17] and exposes
  [`simple_clock()`][__link18]. You can pass a timer-capable clock
  to an API that accepts `SimpleClock`.
* With the `test-util` feature, [`ClockControl::to_simple_clock`][__link19] creates a controlled
  `SimpleClock`. A [`Clock`][__link20] from the same control observes the same time.

A time-only API can accept either clock. For example, [`Stopwatch`][__link21] accepts
any [`AsRef<SimpleClock>`][__link22]:

```rust
use tick::{SimpleClock, Stopwatch};

// A driver-free clock that only retrieves time.
let clock = SimpleClock::new_system();

let _now = clock.system_time();

// `Stopwatch` accepts a `SimpleClock` or a `Clock` (both are `AsRef<SimpleClock>`).
let stopwatch = Stopwatch::start_new(&clock);
let _elapsed = stopwatch.elapsed();
```

## Machine time and human time

Time has two common uses:

* Machine time measures intervals, timeouts, periodic work, and cache
  lifetimes. It also stores and parses timestamps.
* Human time uses calendars, local clock time, and time zones. These values
  can be ambiguous.

This crate focuses on machine time. Use [Jiff][__link23] for parsing, formatting,
calendar arithmetic, local clock time, and time zones.

[`Clock::system_time_as`][__link24] and [`SimpleClock::system_time_as`][__link25] convert the
current time directly to types that implement `TryFrom<SystemTime>`.
Jiff’s [`Timestamp`][__link26] implements this conversion:

```rust
use jiff::Timestamp;
use tick::Clock;

let clock = Clock::new_frozen();
let timestamp: Timestamp = clock.system_time_as();
assert_eq!(timestamp, Timestamp::UNIX_EPOCH);
```

See the `interop_jiff` example for time zones and formatting.

## Thread-aware relocation

All clock types implement [`ThreadAware`][__link27].
Thread-isolated runtimes can use it to isolate timers.

If an [`InactiveClock`][__link28] moves between coordinates
with the same runtime owner, each destination thread gets separate timer
storage. After activation, each thread has an independent [`Clock`][__link29] and
[`ClockDriver`][__link30]. A move to another owner keeps the
existing storage instead.

Relocation does not change a [`ClockControl`][__link31] clock. Its clones share one
controlled time state across threads.

See the [`runtime`][__link32] module documentation for setup examples.

## Testing

Enable `test-util` to use [`ClockControl`][__link33] in tests.

Do not enable `test-util` in production dependencies. Enable it only in
`dev-dependencies`. The feature adds test support that reduces production
performance.

## Examples

### Use `Clock` to retrieve absolute time

A clock returns absolute time as `SystemTime`. See [`Clock`][__link34] for details.

```rust
use std::time::{Duration, SystemTime};

use tick::Clock;

// Read two absolute times.
let time1: SystemTime = clock.system_time();
let time2: SystemTime = clock.system_time();

// The operating system can adjust system time between these calls.
assert!(time1 <= time2);
```

### Use `Clock` to retrieve relative time

A clock provides relative time through [`Clock::instant`][__link35] and [`Stopwatch`][__link36].

```rust
use std::time::{Duration, Instant};

use tick::Clock;

// Use a stopwatch to measure elapsed time.
let stopwatch = clock.stopwatch();
// Perform some operation...
let elapsed: Duration = stopwatch.elapsed();

// Use `Clock::instant` for direct access to monotonic time.
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

### Use `Clock` to create a `Timer`

```rust
use std::time::Duration;

use tick::{Clock, Timer};

// Wait 10 milliseconds before the first timer starts.
clock.delay(Duration::from_millis(10)).await;

let mut timer = Timer::new(clock, Duration::from_millis(1));

for _ in 0..3 {
    timer.next().await;
    // Run periodic work.
}
```

## Features

You can enable these features in `Cargo.toml`:

* `tokio` adds [Tokio][__link37] integration and
  [`Clock::new_tokio`][__link38].
* `test-util` adds [`ClockControl`][__link39] for deterministic tests. It can freeze
  time, advance time, or advance timers automatically. Enable it only in
  `dev-dependencies`.
* `coarse-time` adds [`Stopwatch::start_coarse`][__link40]. This constructor uses a
  lower-overhead, lower-precision instant source on Linux and Windows.
  Other platforms use [`std::time::Instant::now`][__link41].

### Coarse stopwatch

Use a coarse stopwatch only if measurements show that elapsed-time reads are
a performance bottleneck. Values do not decrease, but consecutive reads can
be equal. The platform defines their precision.

```rust
use tick::{SimpleClock, Stopwatch};

let stopwatch = Stopwatch::start_coarse(SimpleClock::new_system());
let elapsed = stopwatch.elapsed();
```

## More examples

The [time examples][__link42]
show more uses of these types.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/tick">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbhtK2lgppMZIbCc11AuRm53Ab1y91Msl0WtcbqsCbpI4bugthZIOCZGppZmZlMS4wLjGCbHRocmVhZF9hd2FyZWYwLjEyLjKCZHRpY2tlMC45LjE
 [__link0]: https://docs.rs/tick/0.9.1/tick/runtime/index.html
 [__link1]: https://docs.rs/tick/0.9.1/tick/?search=ClockControl
 [__link10]: https://docs.rs/tick/0.9.1/tick/?search=Error
 [__link11]: https://docs.rs/tick/0.9.1/tick/runtime/index.html
 [__link12]: https://github.com/microsoft/oxidizer/blob/main/crates/tick/docs/design.md
 [__link13]: https://docs.rs/tick/0.9.1/tick/?search=FutureExt
 [__link14]: https://docs.rs/tick/0.9.1/tick/?search=SimpleClock
 [__link15]: https://docs.rs/tick/0.9.1/tick/?search=SimpleClock::new_system
 [__link16]: https://docs.rs/tick/0.9.1/tick/?search=Clock
 [__link17]: https://doc.rust-lang.org/stable/std/convert/trait.AsRef.html
 [__link18]: https://docs.rs/tick/0.9.1/tick/?search=Clock::simple_clock
 [__link19]: https://docs.rs/tick/0.9.1/tick/?search=ClockControl::to_simple_clock
 [__link2]: https://docs.rs/tick/0.9.1/tick/?search=Clock
 [__link20]: https://docs.rs/tick/0.9.1/tick/?search=Clock
 [__link21]: https://docs.rs/tick/0.9.1/tick/?search=Stopwatch
 [__link22]: https://doc.rust-lang.org/stable/std/convert/trait.AsRef.html
 [__link23]: https://crates.io/crates/jiff
 [__link24]: https://docs.rs/tick/0.9.1/tick/?search=Clock::system_time_as
 [__link25]: https://docs.rs/tick/0.9.1/tick/?search=SimpleClock::system_time_as
 [__link26]: https://docs.rs/jiff/1.0.1/jiff/?search=Timestamp
 [__link27]: https://docs.rs/thread_aware/0.12.2/thread_aware/?search=ThreadAware
 [__link28]: https://docs.rs/tick/0.9.1/tick/?search=runtime::InactiveClock
 [__link29]: https://docs.rs/tick/0.9.1/tick/?search=Clock
 [__link3]: https://docs.rs/tick/0.9.1/tick/?search=Clock
 [__link30]: https://docs.rs/tick/0.9.1/tick/?search=runtime::ClockDriver
 [__link31]: https://docs.rs/tick/0.9.1/tick/?search=ClockControl
 [__link32]: https://docs.rs/tick/0.9.1/tick/runtime/index.html
 [__link33]: https://docs.rs/tick/0.9.1/tick/?search=ClockControl
 [__link34]: https://docs.rs/tick/0.9.1/tick/?search=Clock
 [__link35]: https://docs.rs/tick/0.9.1/tick/?search=Clock::instant
 [__link36]: https://docs.rs/tick/0.9.1/tick/?search=Stopwatch
 [__link37]: https://tokio.rs/
 [__link38]: https://docs.rs/tick/0.9.1/tick/?search=Clock::new_tokio
 [__link39]: https://docs.rs/tick/0.9.1/tick/?search=ClockControl
 [__link4]: https://docs.rs/tick/0.9.1/tick/?search=SimpleClock
 [__link40]: https://docs.rs/tick/0.9.1/tick/?search=Stopwatch::start_coarse
 [__link41]: https://doc.rust-lang.org/stable/std/?search=time::Instant::now
 [__link42]: https://github.com/microsoft/oxidizer/tree/main/crates/tick/examples
 [__link5]: https://doc.rust-lang.org/stable/std/convert/trait.AsRef.html
 [__link6]: https://docs.rs/tick/0.9.1/tick/?search=ClockControl
 [__link7]: https://docs.rs/tick/0.9.1/tick/?search=Stopwatch
 [__link8]: https://docs.rs/tick/0.9.1/tick/?search=Delay
 [__link9]: https://docs.rs/tick/0.9.1/tick/?search=Timer
