<div align="center">
 <img src="./logo.png" alt="Uniflight Logo" width="96">

# Uniflight

[![crate.io](https://img.shields.io/crates/v/uniflight.svg)](https://crates.io/crates/uniflight)
[![docs.rs](https://docs.rs/uniflight/badge.svg)](https://docs.rs/uniflight)
[![MSRV](https://img.shields.io/crates/msrv/uniflight)](https://crates.io/crates/uniflight)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Coalesces duplicate async tasks into a single execution.

This crate provides [`Merger`][__link0], a mechanism for deduplicating concurrent async operations.
When multiple tasks request the same work (identified by a key), only the first task (the
“leader”) performs the actual work while subsequent tasks (the “followers”) wait and receive
a clone of the result.

## When to Use

Use `Merger` when you have expensive or rate-limited operations that may be requested
concurrently with the same parameters:

* **Cache population**: Prevent thundering herd when a cache entry expires
* **API calls**: Deduplicate concurrent requests to the same endpoint
* **Database queries**: Coalesce identical queries issued simultaneously
* **File I/O**: Avoid reading the same file multiple times concurrently

## Example

```rust
use uniflight::Merger;

let group: Merger<String, String> = Merger::new();

// Multiple concurrent calls with the same key will share a single execution.
// Note: you can pass &str directly when the key type is String.
let result = group.execute("user:123", || async {
    // This expensive operation runs only once, even if called concurrently
    "expensive_result".to_string()
}).await;
```

## Flexible Key Types

The [`Merger::execute`][__link1] method accepts keys using [`Borrow`][__link2] semantics, allowing you to pass
borrowed forms of the key type. For example, with `Merger<String, T>`, you can pass `&str`
directly without allocating:

```rust
let merger: Merger<String, i32> = Merger::new();

// Pass &str directly - no need to call .to_string()
merger.execute("my-key", || async { 42 }).await;
```

## Thread-Aware Scoping

`Merger` supports thread-aware scoping via a [`Strategy`][__link3]
type parameter. This controls how the internal state is partitioned across threads/NUMA nodes:

* [`PerProcess`][__link4] (default): Single global state, maximum deduplication
* [`PerNuma`][__link5]: Separate state per NUMA node, NUMA-local memory access
* [`PerCore`][__link6]: Separate state per core, no deduplication (useful for already-partitioned work)

```rust
use uniflight::{Merger, PerNuma};

// NUMA-aware merger - each NUMA node gets its own deduplication scope
let merger: Merger<String, String, PerNuma> = Merger::new_per_numa();
```

## Cancellation and Panic Safety

`Merger` handles task cancellation and panics gracefully:

* If the leader task is cancelled or dropped, a follower becomes the new leader
* If the leader task panics, a follower becomes the new leader and executes its work
* Followers that join before the leader completes receive the cached result

## Memory Management

Completed entries are automatically removed from the internal map when the last caller
finishes. This ensures no stale entries accumulate over time.

## Thread Safety

[`Merger`][__link7] is `Send` and `Sync`, and can be shared across threads. The returned futures
are `Send` when the closure, future, key, and value types are `Send`.

## Performance

Benchmarks comparing `uniflight` against `singleflight-async`:

|Benchmark|uniflight|singleflight-async|Winner|
|---------|---------|------------------|------|
|Single call|777 ns|691 ns|~equal|
|10 concurrent tasks|58 µs|57 µs|~equal|
|100 concurrent tasks|218 µs|219 µs|~equal|
|10 keys × 10 tasks|186 µs|270 µs|uniflight 1.4x|
|Sequential reuse|799 ns|759 ns|~equal|

uniflight’s `DashMap`-based architecture scales well under contention, making it
well-suited for high-concurrency workloads. For single-call scenarios, both libraries
perform similarly (sub-microsecond).


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/uniflight">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG04a0viCY92EG7dKIcfT3ii0G7aPoi1gqOy7Gxzt8DWyGmp1YWSCgmx0aHJlYWRfYXdhcmVlMC42LjCCaXVuaWZsaWdodGUwLjEuMA
 [__link0]: https://docs.rs/uniflight/0.1.0/uniflight/struct.Merger.html
 [__link1]: https://docs.rs/uniflight/0.1.0/uniflight/?search=Merger::execute
 [__link2]: https://doc.rust-lang.org/stable/std/?search=borrow::Borrow
 [__link3]: https://docs.rs/thread_aware/0.6.0/thread_aware/?search=storage::Strategy
 [__link4]: https://docs.rs/thread_aware/0.6.0/thread_aware/?search=PerProcess
 [__link5]: https://docs.rs/thread_aware/0.6.0/thread_aware/?search=PerNuma
 [__link6]: https://docs.rs/thread_aware/0.6.0/thread_aware/?search=PerCore
 [__link7]: https://docs.rs/uniflight/0.1.0/uniflight/struct.Merger.html
