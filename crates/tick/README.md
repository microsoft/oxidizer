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
* [`fmt`][__link11] parses and displays `SystemTime`. Enable `fmt` to use it.
* [`runtime`][__link12] connects clocks to async runtimes.

## Extensions

* [`FutureExt`][__link13] adds timeouts to `Future`.
* [`SystemTimeExt`][__link14] adds display formats to
  [`SystemTime`][__link15].

## Time retrieval without timers

Use [`SimpleClock`][__link16] when an API only reads time. It has no timers, so it
needs no async runtime or driver. [`SimpleClock::new_system`][__link17] creates a
ready-to-use clock that reads operating system time.

Each clock type provides the `SimpleClock` interface:

* [`Clock`][__link18] implements [`AsRef<SimpleClock>`][__link19] and exposes
  [`simple_clock()`][__link20]. You can pass a timer-capable clock
  to an API that accepts `SimpleClock`.
* With the `test-util` feature, [`ClockControl::to_simple_clock`][__link21] creates a controlled
  `SimpleClock`. A [`Clock`][__link22] from the same control observes the same time.

A time-only API can accept either clock. For example, [`Stopwatch`][__link23] accepts
any [`AsRef<SimpleClock>`][__link24]:

```rust
use tick::{SimpleClock, Stopwatch};

// A driver-free clock that only retrieves time.
let clock = SimpleClock::new_system();

let _now = clock.system_time();

// `Stopwatch` accepts a `SimpleClock` or a `Clock` (both are `AsRef<SimpleClock>`).
let stopwatch = Stopwatch::new(&clock);
let _elapsed = stopwatch.elapsed();
```

## Machine time and human time

Time has two common uses:

* Machine time measures intervals, timeouts, periodic work, and cache
  lifetimes. It also stores and parses timestamps.
* Human time uses calendars, local clock time, and time zones. These values
  can be ambiguous.

This crate focuses on machine time. Use [jiff][__link25], [chrono][__link26], or [time][__link27] for
human time. `tick` values can convert to types from these crates. See the
`interop_*` examples.

## Thread-aware relocation

All clock types implement [`ThreadAware`][__link28].
Thread-isolated runtimes can use it to isolate timers.

If an [`InactiveClock`][__link29] moves between coordinates
with the same runtime owner, each destination thread gets separate timer
storage. After activation, each thread has an independent [`Clock`][__link30] and
[`ClockDriver`][__link31]. A move to another owner keeps the
existing storage instead.

Relocation does not change a [`ClockControl`][__link32] clock. Its clones share one
controlled time state across threads.

See the [`runtime`][__link33] module documentation for setup examples.

## Testing

Enable `test-util` to use [`ClockControl`][__link34] in tests.

Do not enable `test-util` in production dependencies. Enable it only in
`dev-dependencies`. The feature adds test support that reduces production
performance.

## Examples

### Use `Clock` to retrieve absolute time

A clock returns absolute time as `SystemTime`. See [`Clock`][__link35] for details.

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

A clock provides relative time through [`Clock::instant`][__link36] and [`Stopwatch`][__link37].

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

* `tokio` adds [Tokio][__link38] integration and
  [`Clock::new_tokio`][__link39].
* `test-util` adds [`ClockControl`][__link40] for deterministic tests. It can freeze
  time, advance time, or advance timers automatically. Enable it only in
  `dev-dependencies`.
* `serde` adds serialization through [serde][__link41].
* `fmt` adds the [`fmt`][__link42] module and formats such as ISO 8601 and RFC 2822.
* `fast-instant` lets each clock clone select a lower-overhead,
  lower-precision instant source on Linux and Windows. Timer scheduling stays
  precise. Other platforms use [`std::time::Instant::now`][__link43]. Precise reads
  remain the default.

### Fast instant contract

Fast instant reads are disabled by default. Each clock clone has its own
setting. Values do not decrease, but consecutive reads can be equal. The
platform defines their precision. A stopwatch keeps the source setting of
its clock.

This option does not affect `system_time()`, controlled clocks, or precise
timer scheduling. On an unsupported platform, it keeps the standard instant
source.

### Using precise and fast clocks together

A component can create a fast clone from an existing [`Clock`][__link44]. It does not
need a separate application dependency. The original clock stays precise:

```rust
use tick::Clock;

fn process(clock: &Clock) {
    let fast_clock = clock.clone().with_fast_instant(true);

    let decision_time = clock.instant();
    let telemetry_time = fast_clock.instant();
    // Use precise time for logic and fast time for debug data.
}
```

## More examples

The [time examples][__link45]
show more uses of these types.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/tick">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbCO8Y53CitZwbOMU0vkTeJdMbvs6eKZq5BxYbgWMTBb-sUqphZIKCbHRocmVhZF9hd2FyZWYwLjEyLjGCZHRpY2tlMC44LjA
 [__link0]: https://docs.rs/tick/0.8.0/tick/runtime/index.html
 [__link1]: https://docs.rs/tick/0.8.0/tick/?search=ClockControl
 [__link10]: https://docs.rs/tick/0.8.0/tick/?search=Error
 [__link11]: https://docs.rs/tick/0.8.0/tick/fmt/index.html
 [__link12]: https://docs.rs/tick/0.8.0/tick/runtime/index.html
 [__link13]: https://docs.rs/tick/0.8.0/tick/?search=FutureExt
 [__link14]: https://docs.rs/tick/0.8.0/tick/?search=SystemTimeExt
 [__link15]: https://doc.rust-lang.org/stable/std/?search=time::SystemTime
 [__link16]: https://docs.rs/tick/0.8.0/tick/?search=SimpleClock
 [__link17]: https://docs.rs/tick/0.8.0/tick/?search=SimpleClock::new_system
 [__link18]: https://docs.rs/tick/0.8.0/tick/?search=Clock
 [__link19]: https://doc.rust-lang.org/stable/std/convert/trait.AsRef.html
 [__link2]: https://docs.rs/tick/0.8.0/tick/?search=Clock
 [__link20]: https://docs.rs/tick/0.8.0/tick/?search=Clock::simple_clock
 [__link21]: https://docs.rs/tick/0.8.0/tick/?search=ClockControl::to_simple_clock
 [__link22]: https://docs.rs/tick/0.8.0/tick/?search=Clock
 [__link23]: https://docs.rs/tick/0.8.0/tick/?search=Stopwatch
 [__link24]: https://doc.rust-lang.org/stable/std/convert/trait.AsRef.html
 [__link25]: https://crates.io/crates/jiff
 [__link26]: https://crates.io/crates/chrono
 [__link27]: https://crates.io/crates/time
 [__link28]: https://docs.rs/thread_aware/0.12.1/thread_aware/?search=ThreadAware
 [__link29]: https://docs.rs/tick/0.8.0/tick/?search=runtime::InactiveClock
 [__link3]: https://docs.rs/tick/0.8.0/tick/?search=Clock
 [__link30]: https://docs.rs/tick/0.8.0/tick/?search=Clock
 [__link31]: https://docs.rs/tick/0.8.0/tick/?search=runtime::ClockDriver
 [__link32]: https://docs.rs/tick/0.8.0/tick/?search=ClockControl
 [__link33]: https://docs.rs/tick/0.8.0/tick/runtime/index.html
 [__link34]: https://docs.rs/tick/0.8.0/tick/?search=ClockControl
 [__link35]: https://docs.rs/tick/0.8.0/tick/?search=Clock
 [__link36]: https://docs.rs/tick/0.8.0/tick/?search=Clock::instant
 [__link37]: https://docs.rs/tick/0.8.0/tick/?search=Stopwatch
 [__link38]: https://tokio.rs/
 [__link39]: https://docs.rs/tick/0.8.0/tick/?search=Clock::new_tokio
 [__link4]: https://docs.rs/tick/0.8.0/tick/?search=SimpleClock
 [__link40]: https://docs.rs/tick/0.8.0/tick/?search=ClockControl
 [__link41]: https://serde.rs/
 [__link42]: https://docs.rs/tick/0.8.0/tick/fmt/index.html
 [__link43]: https://doc.rust-lang.org/stable/std/?search=time::Instant::now
 [__link44]: https://docs.rs/tick/0.8.0/tick/?search=Clock
 [__link45]: https://github.com/microsoft/oxidizer/tree/main/crates/tick/examples
 [__link5]: https://doc.rust-lang.org/stable/std/convert/trait.AsRef.html
 [__link6]: https://docs.rs/tick/0.8.0/tick/?search=ClockControl
 [__link7]: https://docs.rs/tick/0.8.0/tick/?search=Stopwatch
 [__link8]: https://docs.rs/tick/0.8.0/tick/?search=Delay
 [__link9]: https://docs.rs/tick/0.8.0/tick/?search=Timer
