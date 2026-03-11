<div align="center">
 <img src="./logo.png" alt="Cachet Logo" width="96">

# Cachet

[![crate.io](https://img.shields.io/crates/v/cachet.svg)](https://crates.io/crates/cachet)
[![docs.rs](https://docs.rs/cachet/badge.svg)](https://docs.rs/cachet)
[![MSRV](https://img.shields.io/crates/msrv/cachet)](https://crates.io/crates/cachet)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A composable, multi-tier caching library with stampede protection, background
refresh, and built-in OpenTelemetry telemetry.

## Why Multi-Tier Caching?

A single cache is a single point of failure and a capacity ceiling. Multi-tier
caching layers fast, small caches in front of slower, larger ones:

* **L1 (primary)** - an in-process memory cache: microsecond latency, bounded
  capacity, evicts under pressure.
* **L2 (fallback)** - a remote or larger cache: millisecond latency, much larger
  capacity, survives process restarts.

On a miss in L1, `cachet` transparently queries L2 and optionally *promotes* the
value back into L1 so the next request is fast again. The result is lower average
latency, reduced load on the backing store, and resilience when either tier is
temporarily unavailable.

## Why Background Refresh?

TTL-based expiration causes a *synchronous* miss every time an entry ages out:
the next caller blocks while the value is recomputed. Background refresh
(time-to-refresh, TTR) decouples freshness from latency:

* While an entry is still within its TTR, all callers receive the cached value
  immediately (a “refresh hit”).
* Once the TTR elapses, the *next* caller still receives the stale value, but a
  background task is spawned to pull a fresh value from the fallback tier.
* Subsequent callers continue to hit the cache while the refresh happens, so
  latency never spikes.

Use [`TimeToRefresh`][__link0] together with a fallback tier to enable this pattern.

## Cache Stampede Protection

A **cache stampede** (also called a thundering herd) occurs when many concurrent
requests all miss the cache at the same time - for example, after a cold start or
after a popular entry expires. Every request independently computes the value,
spiking load on the backing store.

`cachet` avoids this with *request coalescing* via the [`uniflight`][__link1] crate: when
stampede protection is enabled, all concurrent requests for the same key are merged
so that only one computes the value. The rest wait and share the result, including
any error. Enable it with [`CacheBuilder::stampede_protection`][__link2].

## Flexibility

`cachet` is designed to adapt to your infrastructure rather than the other way
around:

* **Any storage backend** - implement [`CacheTier`][__link3] to plug in Redis, Memcached,
  a database, or any other store.
* **Service middleware** - with the `service` feature, any
  `Service<CacheOperation>` becomes a `CacheTier`, so you can compose retry,
  timeout, and circuit-breaker middleware around your storage using standard Tower
  or `layered` patterns.
* **Dynamic dispatch** - convert a concretely-typed `Cache<K, V, CT>` to
  `Cache<K, V, DynamicCache<K, V>>` via [`Cache::into_dynamic`][__link4] to erase the
  storage type, enabling heterogeneous tier composition at runtime.
* **Configurable promotion** - choose whether, and under what conditions, values
  found in a fallback tier are promoted back into the primary tier
  ([`FallbackPromotionPolicy`][__link5]).
* **Clock injection** - all time-based logic (TTL, TTR, timestamps) goes through
  a [`tick::Clock`][__link6], making caches fully controllable in tests without sleeping.

## Why Use This Instead of Moka/Other Caches?

Moka (and similar crates) are excellent single-tier in-process caches. `cachet`
builds on top of them and adds:

|Feature|Moka|`cachet`|
|-------|----|--------|
|In-process memory cache|✅|✅ (via `cachet_memory`)|
|Multi-tier / fallback|❌|✅|
|Stampede protection|❌|✅|
|Background refresh|❌|✅|
|Service middleware integration|❌|✅|
|OpenTelemetry metrics + logs|❌|✅|
|Pluggable storage backends|❌|✅|
|Clock injection for testing|❌|✅|

If you only need a single in-process cache with no telemetry requirements,
`moka` directly may be simpler. If you need any of the above, `cachet` is the
right choice.

## Major Types

|Type|Description|
|----|-----------|
|[`Cache`][__link7]|The user-facing cache. Wraps any `CacheTier` with `get`, `insert`, `invalidate`, `clear`, `get_or_insert`, `try_get_or_insert`, and `optionally_get_or_insert`.|
|[`CacheBuilder`][__link8]|Builder for `Cache`. Configure storage, TTL, name, telemetry, fallback, promotion policy, stampede protection, and background refresh.|
|[`CacheEntry<V>`][__link9]|A value together with an optional cached-at timestamp and TTL. Returned by all `get` operations.|
|[`CacheTier`][__link10]|The core trait for storage backends. Implement this to add your own storage.|
|[`FallbackPromotionPolicy`][__link11]|Decides whether a value found in a fallback tier is promoted to the primary tier.|
|[`TimeToRefresh`][__link12]|Configures background refresh: how stale an entry must be before a background task refreshes it.|
|[`Error`][__link13]|The error type returned by all fallible cache operations.|

## How Tiers Compose

Tiers are composed at build time using the builder:

```text
Cache::builder::<K, V>(clock)
    .memory()                          // L1: fast in-process store
    .ttl(Duration::from_secs(30))      // entries expire from L1 after 30 s
    .fallback(                         // on L1 miss, consult L2
        Cache::builder::<K, V>(clock)
            .memory()                  // L2: a second in-process store (or a remote service)
            .ttl(Duration::from_secs(300))
    )
    .promotion_policy(FallbackPromotionPolicy::always())  // promote L2 hits into L1
    .refresh(TimeToRefresh::new(Duration::from_secs(20), spawner))  // refresh L1 in background
    .build()
```

On a `get`:

1. Check L1. If hit and not stale, return immediately.
1. If hit but stale (TTR elapsed), return the stale value *and* spawn a background
   task to fetch from L2 and repopulate L1.
1. If miss, check L2. If found, optionally promote to L1, then return.
1. If both miss, return `Ok(None)`.

Invalidation and clear are sent to **all** tiers concurrently.

## Companion Crates

`cachet` is the main entry point. The ecosystem is split into focused crates:

|Crate|Purpose|
|-----|-------|
|[`cachet_tier`][__link14]|Core `CacheTier` trait, `CacheEntry`, `Error`, and `MockCache` for testing.|
|[`cachet_memory`][__link15]|In-process memory cache backed by [moka][__link16] (`TinyLFU` eviction).|
|[`cachet_service`][__link17]|Adapters between the `CacheTier` trait and the `layered::Service` / Tower service patterns.|

You rarely need to depend on companion crates directly - `cachet` re-exports the
most commonly used types from all of them.

## Cargo Features

|Feature|Default|Description|
|-------|-------|-----------|
|`memory`|✅|Enables `InMemoryCache` and the `.memory()` builder method via `cachet_memory`.|
|`metrics`|❌|Enables OpenTelemetry metrics (`cache.event.count`, `cache.operation.duration`, `cache.size`).|
|`logs`|❌|Enables structured `tracing` log events for every cache activity.|
|`service`|❌|Enables `ServiceAdapter`, `CacheServiceExt`, and `CacheOperation`/`CacheResponse` types for service middleware integration.|
|`test-util`|❌|Enables `MockCache`, frozen-clock utilities, and other test helpers.|

## Examples

### Basic In-Memory Cache

```rust
use cachet::{Cache, CacheEntry};
use tick::Clock;

let clock = Clock::new_tokio();
let cache = Cache::builder::<String, i32>(clock)
    .memory()
    .build();

cache.insert("key", CacheEntry::new(42)).await?;
let value = cache.get("key").await?;
assert_eq!(*value.unwrap().value(), 42);
```

### Multi-Tier Cache with Fallback

```rust
use cachet::{Cache, CacheEntry, FallbackPromotionPolicy};
use tick::Clock;
use std::time::Duration;

let clock = Clock::new_tokio();
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
|`cache.operation.duration`|Histogram|s|Operation latency|
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


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/cachet">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG3ejAdIZLsb_GwZgi6EYEwxxG9yhuVfxLZgeG1uVNJAMwXeBYWSGgmZjYWNoZXRlMC4xLjCCbWNhY2hldF9tZW1vcnllMC4xLjCCbmNhY2hldF9zZXJ2aWNlZTAuMS4wgmtjYWNoZXRfdGllcmUwLjEuMIJkdGlja2UwLjIuMYJpdW5pZmxpZ2h0ZTAuMS4w
 [__link0]: https://docs.rs/cachet/0.1.0/cachet/?search=TimeToRefresh
 [__link1]: https://crates.io/crates/uniflight/0.1.0
 [__link10]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=CacheTier
 [__link11]: https://docs.rs/cachet/0.1.0/cachet/?search=FallbackPromotionPolicy
 [__link12]: https://docs.rs/cachet/0.1.0/cachet/?search=TimeToRefresh
 [__link13]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=Error
 [__link14]: https://crates.io/crates/cachet_tier/0.1.0
 [__link15]: https://crates.io/crates/cachet_memory/0.1.0
 [__link16]: https://docs.rs/moka
 [__link17]: https://crates.io/crates/cachet_service/0.1.0
 [__link2]: https://docs.rs/cachet/0.1.0/cachet/?search=CacheBuilder::stampede_protection
 [__link3]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=CacheTier
 [__link4]: https://docs.rs/cachet/0.1.0/cachet/?search=Cache::into_dynamic
 [__link5]: https://docs.rs/cachet/0.1.0/cachet/?search=FallbackPromotionPolicy
 [__link6]: https://docs.rs/tick/0.2.1/tick/?search=Clock
 [__link7]: https://docs.rs/cachet/0.1.0/cachet/?search=Cache
 [__link8]: https://docs.rs/cachet/0.1.0/cachet/?search=CacheBuilder
 [__link9]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=CacheEntry
