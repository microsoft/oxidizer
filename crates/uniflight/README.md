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

This crate provides [`UniFlight`], a mechanism for deduplicating concurrent async operations.
When multiple tasks request the same work (identified by a key), only the first task (the
"leader") performs the actual work while subsequent tasks (the "followers") wait and receive
a clone of the result.

## When to Use

Use `UniFlight` when you have expensive or rate-limited operations that may be requested
concurrently with the same parameters:

- **Cache population**: Prevent thundering herd when a cache entry expires
- **API calls**: Deduplicate concurrent requests to the same endpoint
- **Database queries**: Coalesce identical queries issued simultaneously
- **File I/O**: Avoid reading the same file multiple times concurrently

## Example

```rust
use uniflight::UniFlight;

let group: UniFlight<&str, String> = UniFlight::new();

// Multiple concurrent calls with the same key will share a single execution
let result = group.work("user:123", || async {
    // This expensive operation runs only once, even if called concurrently
    "expensive_result".to_string()
}).await;
```

## Cancellation and Panic Safety

`UniFlight` handles task cancellation and panics gracefully:

- If the leader task is cancelled or dropped, a follower becomes the new leader
- If the leader task panics, a follower becomes the new leader and executes its work
- Followers that join before the leader completes receive the cached result

## Thread Safety

[`UniFlight`] is `Send` and `Sync`, and can be shared across threads. The returned futures
do not require `Send` bounds on the closure or its output.

## Multiple Leaders for Redundancy

By default, `UniFlight` uses a single leader per key. For redundancy scenarios where you want
multiple concurrent attempts at the same operation (using whichever completes first), use
[`UniFlight::with_max_leaders`]:

```rust
use uniflight::UniFlight;

// Allow up to 3 concurrent leaders for redundancy
let group: UniFlight<&str, String> = UniFlight::with_max_leaders(3);

// First 3 concurrent calls become leaders and execute in parallel.
// The first leader to complete stores the result.
// All callers (leaders and followers) receive that result.
let result = group.work("key", || async {
    "result".to_string()
}).await;
```

This is useful when:
- You want fault tolerance through redundant execution
- Network latency varies and you want the fastest response
- You're implementing speculative execution patterns

<!-- cargo-rdme end -->

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
