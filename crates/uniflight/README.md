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
}).await.expect("leader should not panic");
```

## Flexible Key Types

The [`Merger::execute`][__link1] method accepts keys using [`Borrow`][__link2] semantics, allowing you to pass
borrowed forms of the key type. For example, with `Merger<String, T>`, you can pass `&str`
directly without allocating:

```rust
let merger: Merger<String, i32> = Merger::new();

// Pass &str directly - no need to call .to_string()
let result = merger.execute("my-key", || async { 42 }).await;
assert_eq!(result, Ok(42));
```

## Thread-Aware Scoping

`Merger` supports thread-aware scoping via a [`Strategy`][__link3]
type parameter. This controls how the internal state is partitioned across threads/NUMA nodes:

* [`PerProcess`][__link4] (default): Single global state, maximum deduplication
* [`PerNuma`][__link5]: Separate state per NUMA node, NUMA-local memory access
* [`PerCore`][__link6]: Separate state per core, no deduplication (useful for already-partitioned work)

```rust
use uniflight::Merger;
use thread_aware::PerNuma;

// NUMA-aware merger - each NUMA node gets its own deduplication scope
let merger: Merger<String, String, PerNuma> = Merger::new_per_numa();
```

## Cancellation and Panic Handling

`Merger` handles task cancellation and panics explicitly:

* If the leader task is cancelled or dropped, a follower becomes the new leader
* If the leader task panics, followers receive [`LeaderPanicked`][__link7] error with the panic message
* Followers that join before the leader completes receive the value the leader returns

When a panic occurs, followers are notified via the error type rather than silently
retrying. The panic message is captured and available via [`LeaderPanicked::message`][__link8]:

```rust
let merger: Merger<String, String> = Merger::new();
match merger.execute("key", || async { "result".to_string() }).await {
    Ok(value) => println!("got {value}"),
    Err(err) => {
        println!("leader panicked: {}", err.message());
        // Decide whether to retry
    }
}
```

## Memory Management

Completed entries are automatically removed from the internal map when the last caller
finishes. This ensures no stale entries accumulate over time.

## Type Requirements

The value type `T` must implement [`Clone`][__link9] because followers receive a clone of the
leader’s result. The key type `K` must implement [`Hash`][__link10] and [`Eq`][__link11].

## Thread Safety

[`Merger`][__link12] is `Send` and `Sync`, and can be shared across threads. The returned futures
are `Send` when the closure, future, key, and value types are `Send`.

## Performance

Run benchmarks with `cargo bench -p uniflight`. The suite covers:

* `single_call`: Baseline latency with no contention
* `high_contention_100`: 100 concurrent tasks on the same key
* `distributed_10x10`: 10 keys with 10 tasks each

Use `--save-baseline` and `--baseline` flags to track regressions over time.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/uniflight">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEGxgwNFq9VUtfG5xaBNm6U4VGG97W2YkyKkPjG4KVgSbTgdOrYWSCgmx0aHJlYWRfYXdhcmVlMC42LjKCaXVuaWZsaWdodGUwLjEuMA
 [__link0]: https://docs.rs/uniflight/0.1.0/uniflight/struct.Merger.html
 [__link1]: https://docs.rs/uniflight/0.1.0/uniflight/?search=Merger::execute
 [__link10]: https://doc.rust-lang.org/stable/std/?search=hash::Hash
 [__link11]: https://doc.rust-lang.org/stable/std/cmp/trait.Eq.html
 [__link12]: https://docs.rs/uniflight/0.1.0/uniflight/struct.Merger.html
 [__link2]: https://doc.rust-lang.org/stable/std/?search=borrow::Borrow
 [__link3]: https://docs.rs/thread_aware/0.6.2/thread_aware/?search=storage::Strategy
 [__link4]: https://docs.rs/thread_aware/0.6.2/thread_aware/?search=PerProcess
 [__link5]: https://docs.rs/thread_aware/0.6.2/thread_aware/?search=PerNuma
 [__link6]: https://docs.rs/thread_aware/0.6.2/thread_aware/?search=PerCore
 [__link7]: https://docs.rs/uniflight/0.1.0/uniflight/struct.LeaderPanicked.html
 [__link8]: https://docs.rs/uniflight/0.1.0/uniflight/?search=LeaderPanicked::message
 [__link9]: https://doc.rust-lang.org/stable/std/clone/trait.Clone.html
