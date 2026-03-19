<div align="center">
 <img src="./logo.png" alt="Cachet Memory Logo" width="96">

# Cachet Memory

[![crate.io](https://img.shields.io/crates/v/cachet_memory.svg)](https://crates.io/crates/cachet_memory)
[![docs.rs](https://docs.rs/cachet_memory/badge.svg)](https://docs.rs/cachet_memory)
[![MSRV](https://img.shields.io/crates/msrv/cachet_memory)](https://crates.io/crates/cachet_memory)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

High-performance in-memory cache tier.

This crate provides [`InMemoryCache`][__link0], a concurrent in-memory cache with a
`TinyLFU` eviction algorithm for excellent hit rates. Use [`InMemoryCacheBuilder`][__link1]
to configure capacity, TTL, and TTI.

## Quick Start

```rust
use std::time::Duration;

use cachet_memory::InMemoryCacheBuilder;
use cachet_tier::{CacheEntry, CacheTier};


let cache = InMemoryCacheBuilder::<String, i32>::new()
    .max_capacity(1000)
    .time_to_live(Duration::from_secs(300))
    .build()
    .expect("Failed to build cache");

cache
    .insert("key".to_string(), CacheEntry::new(42))
    .await
    .unwrap();
let value = cache.get(&"key".to_string()).await.unwrap();
assert_eq!(*value.unwrap().value(), 42);
```

## Features

* **Capacity limits**: Set maximum entry count with automatic eviction
* **TTL/TTI**: Configure time-to-live and time-to-idle expiration
* **Per-entry TTL**: Honors [`CacheEntry::expires_after`][__link2]
  for per-entry expiration
* **Thread-safe**: Safe for concurrent access from multiple tasks
* **Zero external types**: Builder API keeps implementation details private

## Expiration Behavior

This tier supports three independent expiration mechanisms. When multiple are
active, the **shortest duration wins** - an entry is evicted at the earliest of:

1. The per-entry TTL from [`CacheEntry::expires_after`][__link3]
1. The cache-wide TTL from [`InMemoryCacheBuilder::time_to_live`][__link4]
1. The cache-wide TTI from [`InMemoryCacheBuilder::time_to_idle`][__link5]

This means the builder-level TTL/TTI acts as an **upper bound** on per-entry
TTL. A per-entry TTL longer than the builder TTL will be silently clamped to the
builder value. To give per-entry TTL full control, either leave the builder-level
TTL/TTI unset or set them to a sufficiently high ceiling.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/cachet_memory">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG0e6RRi6uDdyG81AdFYB46FyGxYP2UmYhwH0G2xLXH590rx_YWSCgm1jYWNoZXRfbWVtb3J5ZTAuMS4wgmtjYWNoZXRfdGllcmUwLjEuMA
 [__link0]: https://docs.rs/cachet_memory/0.1.0/cachet_memory/?search=InMemoryCache
 [__link1]: https://docs.rs/cachet_memory/0.1.0/cachet_memory/?search=InMemoryCacheBuilder
 [__link2]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=CacheEntry::expires_after
 [__link3]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=CacheEntry::expires_after
 [__link4]: https://docs.rs/cachet_memory/0.1.0/cachet_memory/?search=InMemoryCacheBuilder::time_to_live
 [__link5]: https://docs.rs/cachet_memory/0.1.0/cachet_memory/?search=InMemoryCacheBuilder::time_to_idle
