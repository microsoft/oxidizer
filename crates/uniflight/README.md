<div align="center">
 <img src="./logo.png" alt="Uniflight Logo" width="128">

# Uniflight

[![crate.io](https://img.shields.io/crates/v/uniflight.svg)](https://crates.io/crates/uniflight)
[![docs.rs](https://docs.rs/uniflight/badge.svg)](https://docs.rs/uniflight)
[![MSRV](https://img.shields.io/crates/msrv/uniflight)](https://crates.io/crates/uniflight)
[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

</div>

* [Summary](#summary)

## Summary

<!-- cargo-rdme start -->

Coalesces duplicate async tasks into a single execution.

This crate provides [`Merger`], a mechanism for deduplicating concurrent async operations.
When multiple tasks request the same work (identified by a key), only the first task (the
"leader") performs the actual work while subsequent tasks (the "followers") wait and receive
a clone of the result.

## When to Use

Use `Merger` when you have expensive or rate-limited operations that may be requested
concurrently with the same parameters:

- **Cache population**: Prevent thundering herd when a cache entry expires
- **API calls**: Deduplicate concurrent requests to the same endpoint
- **Database queries**: Coalesce identical queries issued simultaneously
- **File I/O**: Avoid reading the same file multiple times concurrently

## Example

```rust
use uniflight::Merger;

let group: Merger<&str, String> = Merger::new();

// Multiple concurrent calls with the same key will share a single execution
let result = group.work("user:123", || async {
    // This expensive operation runs only once, even if called concurrently
    "expensive_result".to_string()
}).await;
```

## Cancellation and Panic Safety

`Merger` handles task cancellation and panics gracefully:

- If the leader task is cancelled or dropped, a follower becomes the new leader
- If the leader task panics, a follower becomes the new leader and executes its work
- Followers that join before the leader completes receive the cached result

## Thread Safety

[`Merger`] is `Send` and `Sync`, and can be shared across threads. The returned futures
are `Send` when the closure, future, key, and value types are `Send`.

## Performance

Benchmarks comparing `uniflight` against `singleflight-async` show the following characteristics:

- **Concurrent workloads** (10+ tasks): uniflight is 1.2-1.3x faster, demonstrating better scalability under contention
- **Single calls**: singleflight-async has lower per-call overhead (~2x faster for individual operations)
- **Multiple keys**: uniflight performs 1.3x faster when handling multiple distinct keys concurrently

uniflight's DashMap-based architecture provides excellent scaling properties for high-concurrency scenarios,
making it well-suited for production workloads with concurrent access patterns. For low-contention scenarios
with predominantly single calls, the performance difference is minimal (sub-microsecond range).

<!-- cargo-rdme end -->

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
