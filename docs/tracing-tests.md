# Testing `tracing` Events

Prefer `observed` for new telemetry. Its events do not use the process-global
callsite-interest cache in `tracing-core`. Tests that capture `observed` events
do not need tracing initialization for those events.
The initialization rule below still applies to packages with a direct tracing
dependency and to tests that explicitly exercise tracing through a dependency.

This guide defines the tracing initialization rule for test executables (section 1).
It also gives optional recipes to inspect tracing output (sections 2 and 3).

## 1. Required: initialize tracing so trace-event lines are counted as covered

If a package declares `tracing` as a direct normal or dev-dependency, every Cargo
unit-test and integration-test executable in that package must initialize tracing.
This includes the package's own procedural-macro unit tests.
Build-only dependencies and procedural-macro host dependencies do not qualify a
consumer package.

Also initialize a specific test executable that explicitly exercises or captures
tracing through a dependency, even without a direct tracing dependency.
Using other `testing_aids` helpers does not qualify a target.
Do not add a direct tracing dependency just to qualify for initialization.

Apply the package rule across all declared features and platforms.
If any supported configuration declares the dependency, initialize every target
where its tests run. Do not gate initialization on that dependency's configuration.
An existing whole-target gate can remain because it also disables that target's tests.

Tracing caches interest in event locations within each process.
Without an interested subscriber, tracing can skip field expressions and formatting.
Initialization keeps these paths active for coverage and mutation testing in the
executables covered by this rule.
A dependency's unit-test initialization does not run in a consumer's test executable.
Each executable that explicitly tests tracing therefore needs its own initialization.

A dependency's internal tracing belongs in that dependency's tests.
A transitive tracing dependency alone creates no consumer testing obligation.
Consumer tests do not need initialization just because an operation happens to emit
events inside a dependency.

Initialize `testing_aids` tracing before any test runs, via the
`testing_aids::init_tracing!()` macro. It expands to a `#[ctor::ctor]`
process-initialization function that runs before `main`.
The invoking crate needs a dev-dependency on [`ctor`](https://docs.rs/ctor) and a
path dev-dependency on `testing_aids`.
Keep these dependencies unconditional across features and platforms.
Where the macro goes depends on the binary kind.

**Unit-test binary** (library, binary, or procedural-macro target) - add this at its
Cargo target root, gated on `test`:

```rust
#[cfg(test)]
testing_aids::init_tracing!();
```

**Integration-test binaries** (`tests/*.rs`) link the library's normal (non-test)
build, in which `#[cfg(test)]` items - including the crate-root initialization - are
excluded. So the crate-root initialization never runs for them. Invoke the macro
directly in each integration-test target root covered by the initialization rule.
This includes nested roots such as `tests/io/main.rs` and explicit `[[test]]` paths:

```rust
testing_aids::init_tracing!();
```

Do not add initialization to normal library builds or production startup.
Examples, benchmarks, doctests, and compile-only fixtures are not covered by this
unit-test and integration-test rule.
The `ctor` expansion uses unsafe attributes. In a crate that forbids unsafe code,
use `#![cfg_attr(not(test), forbid(unsafe_code))]` to preserve the production restriction.

The initialization is idempotent. If you forget it, the `testing_aids` tracing helpers
panic with a pointer to this guide rather than failing silently.

The initialized subscriber stays interested in every level for the process lifetime.
**Silent means discarding output after formatting, not skipping formatting.** Field
expressions, `Debug` and `Display` implementations, and message formatting all execute
synchronously, including fields supplied when a span is created or subsequently
recorded with `Span::record`. Formatting panics propagate to the emitting test even
when no output sink is enabled. Partial event text from a panicking formatter is
discarded rather than leaking into a later event or capture.

Span fields are formatted before locking their stored state, so their formatters
can emit nested events without deadlocking. A failed recording leaves the previous
span fields intact and does not poison the span's lock.

This also applies during mutation testing: stdout and file output are suppressed,
but field expressions and formatting still run, and explicitly requested buffers
still capture output for assertions.

Tests that deliberately disable tracing, require no subscriber, or own a custom
process-global subscriber are exceptions. Give each scenario a separate Cargo test
target and document its purpose at the target root.
Do not initialize the shared subscriber in those targets.
See `crates/cachet/tests/no_subscriber.rs` for a no-subscriber example.
Use Cargo target isolation, not subprocesses launched by tests.

### Auditing target roots

Use `cargo metadata --format-version 1 --all-features` without `--filter-platform`
to include optional and target-specific dependency declarations.
Read each workspace package's `dependencies` array.
Select direct `tracing` declarations whose `kind` is normal (`null`) or `dev`.
Do not traverse dependency graphs or select build-only declarations.

Enumerate workspace `packages[].targets[]` with `test = true` and a library,
binary, procedural-macro, or integration-test kind.
Use each target's `src_path`, not a `tests/*.rs` filename search.
For every target in a selected package, inspect its root for initialization or a
documented exception. Also inspect the gates on the initializer and the helper dependencies.
An integration executable cannot inherit a library's `#[cfg(test)]` initializer.

Inspect the remaining tests for explicit tracing use or capture through dependencies.
Follow nested modules, generated test inputs, and `include!` files, not just target roots.
Initialize each target that explicitly tests tracing.
Do not treat a telemetry test for another library, such as `observed`, as a tracing test.

If features are mutually exclusive, collect metadata for each supported combination
and audit the union instead of relying on `--all-features`.
The dependency audit establishes scope. Test execution still needs the supported
platform and feature matrix in CI.

## 2. Optional: write events to stdout, a file, or a buffer (process-global)

*Do this only if you want to see or assert on `tracing` output across all threads.*
These helpers route events through the process-global subscriber, so they affect
all threads: the capture buffer records events emitted from *any* thread. Because of
this, if a test binary uses these helpers at all, **every test in that binary MUST be
`#[serial]`** - not just the ones that call the helpers - so that no other test runs
concurrently and emits into the shared buffer.

Events are formatted once and their rendered text is routed to each active sink.
Output has no timestamps or ANSI colors; stdout receives INFO and above, while file
and buffer output receive every level. Span fields recorded before a sink is enabled
remain available in the context of later captured events.

```rust
use testing_aids::tracing_logs;

// To stdout only (INFO and above):
tracing_logs::write_to_stdout();

// To stdout and a file (file captures all levels), until the guard drops:
let _guard = tracing_logs::write_to_stdout_and_file("my-test.log");

// To stdout and an in-memory buffer, one entry per line:
let guard = tracing_logs::write_to_stdout_and_buffer();
run_the_operation();
let lines = guard.into_inner(); // detaches and returns Vec<String>
assert!(lines.iter().any(|line| line.contains("cache.get")));
```

For events emitted asynchronously (for example, on a background worker), poll
`snapshot()`, which reads the captured lines so far without detaching the buffer.
`crates/cachet/tests/eviction.rs` is a live example: it polls `snapshot()` until a
`moka` eviction listener emits its event on a background thread.

```rust
use std::time::{Duration, Instant};

use serial_test::serial;
use testing_aids::tracing_logs::write_to_stdout_and_buffer;

#[test]
#[serial]
fn emits_event_from_background_thread() {
    let guard = write_to_stdout_and_buffer();

    trigger_the_cross_thread_operation();

    // The event is emitted on a background thread, so poll `snapshot()` until it
    // appears rather than reading once - the operation may return before the
    // background thread emits.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if guard.snapshot().iter().any(|line| line.contains("cache.evict")) {
            break;
        }
        assert!(Instant::now() < deadline, "eviction event was not emitted in time");
        std::thread::sleep(Duration::from_millis(10));
    }
}
```

## 3. Optional: capture events on a single thread (unit tests)

*Do this only if you want to assert on `tracing` output emitted on the current
thread.* Use `testing_aids::tracing_logs::Capture` with `set_default`. Capture is
thread-local: it installs a subscriber for the current thread only (`set_default`),
so it needs no `#[serial]` and touches no process-global `tracing` state. Beyond the
one-time process-init call from section 1, a unit test MUST NOT install its own
global subscriber (for example via `set_global_default`); it must scope capture to
its own thread with this form.

```rust
use tracing_subscriber::util::SubscriberInitExt;
use testing_aids::tracing_logs::Capture;

#[test]
fn emits_operation_event() {
    let capture = Capture::new();
    let _guard = capture.subscriber().set_default();

    run_the_logging_operation();

    capture.assert_contains("cache.get");
}
```
