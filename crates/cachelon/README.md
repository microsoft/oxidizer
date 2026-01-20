<div align="center">
 <img src="./logo.png" alt="Cachelon Logo" width="96">

# Cachelon

[![crate.io](https://img.shields.io/crates/v/cachelon.svg)](https://crates.io/crates/cachelon)
[![docs.rs](https://docs.rs/cachelon/badge.svg)](https://docs.rs/cachelon)
[![MSRV](https://img.shields.io/crates/msrv/cachelon)](https://crates.io/crates/cachelon)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Flexible multi-tier caching with telemetry and TTL support.

This crate provides a composable cache system with:

* Type-safe cache builders for single and multi-tier caches
* Built-in OpenTelemetry metrics and logging
* Per-entry and tier-level TTL expiration
* Fallback cache hierarchies with configurable promotion policies
* Background refresh with stampede protection

## Examples

### Basic In-Memory Cache

```rust
use cachelon::{Cache, CacheEntry};
use tick::Clock;

let clock = Clock::new_frozen();
let cache = Cache::builder::<String, i32>(clock)
    .memory()
    .build();

cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
let value = cache.get(&"key".to_string()).await;
assert_eq!(*value.unwrap().value(), 42);
```

### Multi-Tier Cache with Fallback

```rust
use cachelon::{Cache, CacheEntry, FallbackPromotionPolicy};
use tick::Clock;
use std::time::Duration;

let clock = Clock::new_frozen();
let l2 = Cache::builder::<String, String>(clock.clone()).memory();

let cache = Cache::builder::<String, String>(clock)
    .memory()
    .ttl(Duration::from_secs(60))
    .fallback(l2)
    .promotion_policy(FallbackPromotionPolicy::always())
    .build();
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/cachelon">source code</a>.
</sub>

