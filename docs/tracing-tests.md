# Testing `tracing` Events

> **Prefer `observed` for telemetry.** Emitting telemetry through the `observed`
> package sidesteps everything described here: its events are not subject to the
> `tracing-core` process-global callsite-interest cache, so capturing and asserting
> on them just works with no fallback subscriber, no `#[serial]`, and no
> per-binary constructor. Reach for raw `tracing` events only when you specifically
> need them; otherwise use `observed` and skip this guide.

This repository has one **mandatory requirement** for any test binary that touches
`tracing` (section 1), followed by two optional **how-to** recipes for when you want
to inspect `tracing` output (sections 2 and 3).

## 1. Required: initialize tracing so trace-event lines are counted as covered

**This is mandatory for every test binary that emits or inspects `tracing` events,
whether or not the test inspects the output.** If you skip it, `tracing` event
emission lines (and the field expressions inside them, such as
`cache.duration_ns = duration.as_nanos()`) may be reported as **lacking test
coverage even though they execute during tests** - the coverage miss is
non-deterministic and depends on test scheduling.

Install the `testing_aids` fallback before any test runs, via a constructor. Where
the constructor goes depends on the binary kind.

**Unit-test binary** (`#[cfg(test)]` code under `src/`) — add this at the crate
root, gated on `test`:

```rust
#[cfg(test)]
#[ctor::ctor(unsafe)]
fn init_test_tracing() {
    testing_aids::tracing::initialize();
}
```

**Integration-test binaries** (`tests/*.rs`) — `cfg(test)` is false here, so the
crate-root constructor does not run. Add an ungated file-level constructor to each
`tests/*.rs` file that emits or inspects `tracing`:

```rust
#[ctor::ctor(unsafe)]
fn init_test_tracing() {
    testing_aids::tracing::initialize();
}
```

`initialize()` is idempotent. If you forget it, the `testing_aids` tracing helpers
panic with a pointer to this guide rather than failing silently.

> A binary that deliberately runs with no subscriber (to exercise the
> no-subscriber code paths) must NOT install the fallback and must own its own
> binary; see `crates/cachet/tests/no_subscriber.rs`.

## 2. Optional: write events to stdout, a file, or a buffer (process-global)

*Do this only if you want to see or assert on `tracing` output across all threads.*
These helpers route events through the process-global subscriber, so they affect
all threads. Capture is process-global: **every test in a binary that uses them
MUST be `#[serial]`.**

```rust
use testing_aids::tracing;

// To stdout only (INFO and above):
tracing::write_to_stdout();

// To stdout and a file (file captures all levels), until the guard drops:
let _guard = tracing::write_to_stdout_and_file("my-test.log");

// To stdout and an in-memory buffer, one entry per line:
let guard = tracing::write_to_stdout_and_buffer();
run_the_operation();
let lines = guard.into_inner(); // detaches and returns Vec<String>
assert!(lines.iter().any(|line| line.contains("cache.get")));
```

For events emitted asynchronously (for example, on a background worker), poll
`snapshot()`, which reads the captured lines so far without detaching the buffer.
`crates/cachet/tests/eviction.rs` is a live example: it polls `snapshot()` until a
`moka` eviction listener emits its event on a background thread.

```rust
use serial_test::serial;
use testing_aids::tracing::write_to_stdout_and_buffer;

#[test]
#[serial]
fn emits_event_from_background_thread() {
    let guard = write_to_stdout_and_buffer();

    run_the_cross_thread_operation();

    let lines = guard.into_inner();
    assert!(lines.iter().any(|line| line.contains("cache.evict")));
}
```

## 3. Optional: capture events on a single thread (unit tests)

*Do this only if you want to assert on `tracing` output emitted on the current
thread.* Use `testing_aids::tracing::Capture` with `set_default`. This is
thread-local, so it needs no `#[serial]` and does not touch `tracing` global state.
Unit tests MUST use this form and MUST NOT install a global subscriber.

```rust
use tracing_subscriber::util::SubscriberInitExt;
use testing_aids::tracing::Capture;

#[test]
fn emits_operation_event() {
    let capture = Capture::new();
    let _guard = capture.subscriber().set_default();

    run_the_logging_operation();

    capture.assert_contains("cache.get");
}
```
