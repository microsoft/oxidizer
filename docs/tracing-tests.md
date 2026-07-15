# Testing `tracing` Events

> **Prefer `observed` for telemetry.** Emitting telemetry through the `observed`
> package sidesteps everything described here: its events are not subject to the
> `tracing-core` process-global callsite-interest cache, so capturing and asserting
> on them just works with no fallback subscriber, no `#[serial]`, and no
> per-binary constructor. Reach for raw `tracing` events only when you specifically
> need them; otherwise use `observed` and skip this guide.

This is a how-to guide for testing raw `tracing` output in this repository. Because
`tracing` subscribers and the `tracing-core` callsite-interest cache are
process-global state, follow these recipes exactly; they keep capture and coverage
deterministic without relying on the test runner's process isolation.

## 1. Initialize tracing in every test binary that touches `tracing`

Every test binary that emits or inspects `tracing` events MUST install the
`testing_aids` fallback before any test runs, via a constructor. This is what makes
`tracing` event coverage deterministic. Where the constructor goes depends on the
binary kind.

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

## 2. Write events to stdout, a file, or a buffer (process-global)

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

## 3. Capture events on a single thread (unit tests)

To capture events emitted on the current thread only, use
`testing_aids::tracing::Capture` with `set_default`. This is thread-local, so it
needs no `#[serial]` and does not touch `tracing` global state. Unit tests MUST use
this form and MUST NOT install a global subscriber.

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
