# Testing `tracing` Events

> **Prefer `observed` for telemetry.** Emitting telemetry through the `observed`
> package sidesteps everything described here: its events are not subject to the
> `tracing-core` process-global callsite-interest cache, so capturing and asserting
> on them just works with no fallback subscriber, no `#[serial]`, and no
> per-binary constructor. Reach for raw `tracing` events only when you specifically
> need them; otherwise use `observed` and skip this guide.

This is the guide for how to test `tracing` output in this repository. It exists
because `tracing` subscribers and the `tracing-core` callsite-interest cache are
process-global state. Interacting with them naively causes cross-test pollution
that is invisible in isolated runs but corrupts assertions and coverage when many
tests share a process.

## The hazard

`tracing-core` caches, process-wide and permanently, whether each log callsite is
"interested". That decision is made lazily the first time a callsite is reached,
and when few subscribers are registered it is derived from whichever thread
happened to reach the callsite first. If that thread had no subscriber, the
callsite is cached as disabled forever: its field expressions are never evaluated
again, from any thread, regardless of what subscriber a later test installs.

Consequences of ignoring this:

- A later test that installs a capture subscriber sees an empty buffer and fails.
- Coverage of the field-evaluating log statement disappears, non-deterministically.

Neither symptom reproduces reliably, because the outcome depends on test
scheduling order within the shared binary. We do not rely on the test runner's
process-isolation for correctness; isolation is expressed explicitly in the code.

## The fix: an always-interested fallback

Every test binary that emits or inspects `tracing` events installs one silent,
always-interested global subscriber before any test runs. Because that subscriber
is present from the very first callsite hit and is interested in every callsite at
every level, `tracing-core` can never cache a callsite as disabled. The emission
paths therefore always execute, so coverage and per-thread capture become
deterministic regardless of test order.

The fallback is silent: it produces no output on its own. It only guarantees that
callsites stay enabled. Install it with a constructor that runs before `main`.

Each test binary is a separate process with its own callsite-interest cache, so
the fallback must be installed in *every* binary that emits or inspects `tracing`.
Where it goes depends on the binary kind:

- **The crate's own unit-test binary** (`#[cfg(test)]` code under `src/`): add the
  constructor at the crate root, gated on `test`. `#[cfg(test)]` is active only
  when the crate is compiled as its own test harness, which is exactly this
  binary.

  ```rust
  #[cfg(test)]
  #[ctor::ctor(unsafe)]
  fn init_test_tracing() {
      testing_aids::tracing::initialize();
  }
  ```

- **Integration-test binaries** (`tests/*.rs`): these link the library compiled as
  a normal dependency, where `cfg(test)` is *false*, so the crate-root constructor
  above does NOT run in them. An integration binary that emits or inspects
  `tracing` must install the fallback itself with an ungated file-level
  constructor:

  ```rust
  #[ctor::ctor(unsafe)]
  fn init_test_tracing() {
      testing_aids::tracing::initialize();
  }
  ```

  No `#[cfg(test)]` gate here: an integration file only ever compiles into a test
  binary. A binary that deliberately runs with no subscriber (to exercise the
  no-subscriber code paths) must NOT install the fallback and must own its own
  binary; see `crates/cachet/tests/no_subscriber.rs`.

`testing_aids::tracing::initialize()` is idempotent, so multiple callers are harmless.

The `testing_aids` tracing helpers do not install the fallback lazily: doing so
would be too late, because an earlier emission on a subscriber-less thread could
already have poisoned a callsite. Instead they *assert* that `testing_aids::tracing::initialize()`
has already run. `Capture::subscriber()` and the `write_to_stdout*` helpers panic
with a pointer to this guide if the constructor is missing, so a forgotten
constructor fails loudly and deterministically rather than causing a flaky miss.

## Principles

- Every test binary that emits or inspects `tracing` events MUST install the
  silent always-interested fallback, placed according to the binary kind above.
- Unit tests MAY inspect `tracing` output, but only through a thread-local
  subscriber. They MUST NOT touch `tracing` global state: no `set_global_default`,
  no installing a global subscriber (beyond the shared fallback).
- Because unit tests run in parallel and share a process, capture that requires a
  *global* or *multi-threaded* subscriber (for example, events emitted on a
  background thread) is not available to them. Such tests MUST live in their own
  integration-test binary and be annotated `#[serial]`, using the global capture
  bridge below.

## Thread-local capture in unit tests

Use `testing_aids::tracing::Capture` with `set_default` to scope capture to the current
thread. This composes safely with the fallback: the capture subscriber shadows the
fallback on its own thread, and the returned guard restores the fallback on drop.

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

## Global capture in serial integration tests

When capture must observe events from other threads, use the process-global
`testing_aids::tracing::write_to_stdout_and_buffer()` bridge. It tees emitted lines to stdout
and into a buffer, returning a guard; `into_inner()` detaches the buffer and
returns the captured lines. Capture is process-global, so every test in the binary
MUST be `#[serial]`.

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
