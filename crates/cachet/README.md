# cachet ![License: MIT](https://img.shields.io/badge/license-MIT-blue) [![cachet on crates.io](https://img.shields.io/crates/v/cachet)](https://crates.io/crates/cachet) [![cachet on docs.rs](https://docs.rs/cachet/badge.svg)](https://docs.rs/cachet) [![Source Code Repository](https://img.shields.io/badge/Code-On%20GitHub-blue?logo=GitHub)](https://github.com/microsoft/oxidizer/tree/main/crates/cachet) ![Rust Version: 1.88.0](https://img.shields.io/badge/rustc-1.88.0-orange.svg)

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
use cachet::{Cache, CacheEntry};
use tick::Clock;

let clock = Clock::new_frozen();
let cache = Cache::builder::<String, i32>(clock)
    .memory()
    .build();

cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;
let value = cache.get(&"key".to_string()).await?;
assert_eq!(*value.unwrap().value(), 42);
```

### Multi-Tier Cache with Fallback

```rust
use cachet::{Cache, CacheEntry, FallbackPromotionPolicy};
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

## Telemetry

Enable with `metrics` and/or `logs` features. Configure via `.use_metrics()` and `.use_logs()`.

### Metrics (OpenTelemetry)

|Metric|Type|Unit|Description|
|------|----|----|-----------|
|`cache.event.count`|Counter|event|Cache operation events|
|`cache.operation.duration_ns`|Histogram|s|Operation latency|
|`cache.size`|Gauge|entry|Current entry count|

**Attributes:** `cache.name`, `cache.operation`, `cache.activity`

**Operations:** `cache.get`, `cache.insert`, `cache.invalidate`, `cache.clear`

**Activities:** `cache.hit`, `cache.miss`, `cache.expired`, `cache.inserted`,
`cache.invalidated`, `cache.refresh_hit`, `cache.refresh_miss`,
`cache.fallback_promotion`, `cache.error`, `cache.ok`

### Logs (tracing)

Event name: `cache.event` with fields `cache.name`, `cache.operation`,
`cache.activity`, `cache.duration_ns`.

**Levels:** DEBUG (hit/miss/ok), INFO (expired/inserted/invalidated/refresh), ERROR (error)
