# Testing `tracing` Events

Prefer `observed` for new telemetry. Its events do not use the process-global
callsite-interest cache in `tracing-core`. Tests that capture `observed` events
do not need tracing initialization for those events.
The dependency rule below still applies if the executable also depends on `tracing`.

This repository has one initialization rule for test executables that depend on
`tracing` (section 1), followed by optional recipes for when you want
to inspect `tracing` output (sections 2 and 3).

## 1. Required: initialize tracing so trace-event lines are counted as covered

For a test executable, its runtime dependency graph contains its dependencies
and their dependencies.
Every Cargo unit-test or integration-test executable whose runtime dependency graph
contains `tracing`, directly or transitively, must initialize tracing before tests run.
The rule applies even when the target does not call tracing or inspect logs itself.

Start with the target package's normal and dev-dependencies, then follow only normal
dependency edges. Do not follow dependencies' dev-dependencies, build-dependencies,
or procedural-macro host dependencies. Those graphs do not run in the test process.
For a procedural-macro package's own unit tests, start with that package as the root.

Use the union of all supported features and platforms, including supported custom
configurations such as `cfg(loom)`. If any configuration reaches `tracing`, initialize
the target in every configuration where its tests run. Do not gate the initializer
on the feature or platform that introduces tracing. An existing whole-target gate
can remain because it also disables that target's tests.

This rule keeps coverage and mutation testing independent of test order.
Without an interested subscriber, tracing can skip field expressions and formatting
even when tests execute the surrounding code.
Dependency changes require a fresh audit of affected test targets, not a search
for direct tracing calls.

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
directly in each integration-test target root covered by the dependency rule.
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
to include optional and target-specific dependency edges.
Read `resolve.nodes[].deps[].dep_kinds` to distinguish normal, dev, and build edges.
Apply the traversal rule above rather than following every edge in the resolved graph.

Enumerate workspace `packages[].targets[]` with `test = true` and a library,
binary, procedural-macro, or integration-test kind.
Use each target's `src_path`, not a `tests/*.rs` filename search.
For every target that reaches tracing, inspect its root for initialization or a
documented exception. Also inspect the gates on the initializer and the helper dependencies.
An integration executable cannot inherit a library's `#[cfg(test)]` initializer.

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
