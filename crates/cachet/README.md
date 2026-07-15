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
refresh, and structured telemetry.

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
* **Dynamic dispatch** - the builder type-erases the storage tier into a
  [`DynamicCache<K, V>`][__link4], so all builders produce the same `Cache<K, V>`
  output type regardless of the underlying storage or tier composition.
* **Configurable insert policy** - choose whether, and under what conditions,
  values are inserted into a tier ([`InsertPolicy`][__link5]).
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
|Structured telemetry|❌|✅|
|Pluggable storage backends|❌|✅|
|Clock injection for testing|❌|✅|

If you only need a single in-process cache with no telemetry requirements,
`moka` directly may be simpler. If you need any of the above, `cachet` is the
right choice.

## Major Types

|Type|Description|
|----|-----------|
|[`Cache`][__link7]|The user-facing cache. Wraps any `CacheTier` with `get`, `insert`, `invalidate`, `clear`, `get_or_insert`, `try_get_or_insert`, and `optionally_get_or_insert`.|
|[`CacheBuilder`][__link8]|Builder for `Cache`. Configure storage, TTL, name, telemetry, fallback, insert policy, stampede protection, and background refresh.|
|[`CacheEntry<V>`][__link9]|A value together with an optional cached-at timestamp and TTL. Returned by all `get` operations.|
|[`CacheTier`][__link10]|The core trait for storage backends. Implement this to add your own storage.|
|[`InsertPolicy`][__link11]|Decides whether a value should be inserted into a tier.|
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
    .insert_policy(InsertPolicy::always())  // control when values are inserted into L1
    .time_to_refresh(TimeToRefresh::new(Duration::from_secs(20), spawner))  // refresh L1 in background
    .build()
```

On a `get`:

1. Check L1. If hit and not stale, return immediately.
1. If hit but stale (TTR elapsed), return the stale value *and* spawn a background
   task to fetch from L2 and repopulate L1.
1. If miss or expired (TTL elapsed), check L2. If found, optionally promote to L1,
   then return.
1. If both miss, return `Ok(None)`.

**Note:** expired entries are not automatically removed from storage. The wrapper
uses lazy expiration - it returns `None` but leaves cleanup to the storage
backend (e.g. moka built-in eviction).

TO-DO add an `ExpirationPolicy` that would make this configurable.

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
|`logs`|❌|Enables structured `tracing` log events for every cache operation. Subscribe via [`telemetry::attributes`][__link18] constants.|
|`service`|❌|Enables `ServiceAdapter`, `CacheServiceExt`, and `CacheOperation`/`CacheResponse` types for service middleware integration.|
|`serialize`|❌|Enables `.serialize()` on builders for automatic postcard serialization of keys and values to `BytesView`.|
|`encrypt`|❌|Enables `.encrypt_with(cipher)` on serialized builders and the `AeadCipher` trait for authenticated value encryption with a caller-supplied cipher.|
|`test-util`|❌|Enables `MockCache`, frozen-clock utilities, and other test helpers.|

## Examples

### Basic In-Memory Cache

```rust
use cachet::{Cache, CacheEntry};
use tick::Clock;

let clock = Clock::new_tokio();
let cache: Cache<String, i32> = Cache::builder(clock).memory().build();

cache.insert("key".to_string(), CacheEntry::new(42)).await?;
let value = cache.get("key").await?;
assert_eq!(*value.unwrap().value(), 42);
```

### Multi-Tier Cache with Fallback

```rust
use std::time::Duration;

use cachet::Cache;
use tick::Clock;

let clock = Clock::new_tokio();
let l2 = Cache::builder::<String, String>(clock.clone()).memory();

let cache = Cache::builder::<String, String>(clock)
    .memory()
    .ttl(Duration::from_secs(60))
    .fallback(l2)
    .build();
```

### Serialization Boundary

When a fallback tier operates on serialized bytes (e.g., Redis), use `.serialize()`
to add a postcard serialization boundary. Keys and values are automatically serialized
to [`BytesView`][__link19] before reaching the fallback tier, and
deserialized on the way back.

```rust
use cachet::{Cache, FallbackPromotionPolicy};
use tick::Clock;

let clock = Clock::new_tokio();
let remote = Cache::builder::<bytesbuf::BytesView, bytesbuf::BytesView>(clock.clone()).memory();

let cache = Cache::builder::<String, String>(clock)
    .memory()
    .serialize()
    .fallback(remote)
    .promotion_policy(FallbackPromotionPolicy::always())
    .build();

// Keys and values are String on the outside, BytesView in the fallback tier.
cache.insert("key".to_string(), "value".to_string()).await?;
```

### Encryption Boundary

With the `encrypt` feature, chain `.encrypt_with(cipher)` after `.serialize()` to
encrypt values with a caller-supplied `AeadCipher` before they reach the fallback
tier. The cachet crate ships only the encryption *mechanism* — it has **no
cryptographic dependency of its own**, so you plug in a cipher backed by whichever
approved cryptographic library your project mandates. The cipher receives each
value’s storage key as associated data and must authenticate it, which
cryptographically binds every value to its key.

Only values are encrypted: keys are left serialized-but-unencrypted so they remain
deterministic and can be looked up — so do not place secrets or PII in cache keys.
A stored value that fails to decrypt (corrupt, truncated, wrong key, tampered, or
relocated to a different key) is treated as a cache miss and emits a
`cache.decrypt_failed` telemetry event.

```rust
use cachet::Cache;
use tick::Clock;

let clock = Clock::new_tokio();
let remote = Cache::builder::<bytesbuf::BytesView, bytesbuf::BytesView>(clock.clone()).memory();

let cache = Cache::builder::<String, String>(clock)
    .memory()
    .serialize()
    .encrypt_with(my_cipher) // any `AeadCipher` implementation
    .fallback(remote)
    .build();

cache.insert("key".to_string(), "value".to_string()).await?;
```

#### Example: a `SymCrypt`-backed AES-256-GCM cipher

[SymCrypt][__link20] is a FIPS-certifiable,
SDL-approved cryptographic library. The following `AeadCipher` implementation wraps
it using the [`symcrypt`][__link21] crate; it stores each
value as `nonce || ciphertext || tag` with a fresh random 96-bit nonce and
authenticates the storage key as associated data. It is shown here as a reference
rather than shipped as a compiled feature, because `SymCrypt` requires the native
library to be present at build and run time. Add `symcrypt` and `getrandom` to your
own crate to use it.

```rust
use bytesbuf::BytesView;
use cachet::{AeadCipher, DecodeOutcome, Error};
use symcrypt::cipher::BlockCipherType;
use symcrypt::gcm::GcmExpandedKey;

const NONCE_SIZE: usize = 12;
const TAG_SIZE: usize = 16;

pub struct Aes256GcmCipher {
    key: GcmExpandedKey,
}

impl Aes256GcmCipher {
    pub fn new(key: &[u8; 32]) -> Self {
        let key = GcmExpandedKey::new(key, BlockCipherType::AesBlock)
            .expect("AES-256-GCM key expansion cannot fail for a valid 32-byte key");
        Self { key }
    }
}

impl AeadCipher for Aes256GcmCipher {
    fn encrypt(&self, aad: &[u8], plaintext: &BytesView) -> Result<BytesView, Error> {
        let mut nonce = [0u8; NONCE_SIZE];
        getrandom::fill(&mut nonce).map_err(|e| Error::from_message(format!("nonce: {e}")))?;

        // Assemble `nonce || plaintext || tag`, copying the plaintext in once, then
        // encrypt the ciphertext region in place and write the tag into the tail.
        let plaintext_len = plaintext.len();
        let mut result = vec![0u8; NONCE_SIZE + plaintext_len + TAG_SIZE];
        result[..NONCE_SIZE].copy_from_slice(&nonce);
        let mut offset = NONCE_SIZE;
        for (slice, _) in plaintext.slices() {
            result[offset..offset + slice.len()].copy_from_slice(slice);
            offset += slice.len();
        }
        let (head, tag) = result.split_at_mut(NONCE_SIZE + plaintext_len);
        self.key.encrypt_in_place(&nonce, aad, &mut head[NONCE_SIZE..], tag);
        Ok(result.into())
    }

    fn decrypt(&self, aad: &[u8], ciphertext: &BytesView) -> Result<DecodeOutcome<BytesView>, Error> {
        let bytes = ciphertext.to_vec();
        if bytes.len() < NONCE_SIZE + TAG_SIZE {
            return Ok(DecodeOutcome::SoftFailure("ciphertext too short"));
        }
        let (nonce, rest) = bytes.split_at(NONCE_SIZE);
        let (body, tag) = rest.split_at(rest.len() - TAG_SIZE);
        let nonce: &[u8; NONCE_SIZE] = nonce.try_into().expect("exactly 12 bytes");

        let mut buffer = body.to_vec();
        match self.key.decrypt_in_place(nonce, aad, &mut buffer, tag) {
            // Any authentication failure is a soft failure: the entry reads as a miss.
            Ok(()) => Ok(DecodeOutcome::Value(buffer.into())),
            Err(_) => Ok(DecodeOutcome::SoftFailure("AES-GCM decryption failed")),
        }
    }
}
```

Because each encryption uses a fresh random 96-bit nonce, rotate the key
periodically under extreme write volumes to stay well within the birthday bound.

## Telemetry

Cachet provides two complementary telemetry channels:

### Tracing events

Enable with the `logs` feature and `.enable_logs()` on the cache builder.
Each tier outcome and operation completion emits a structured [`tracing`][__link22] event.

**Tier events** carry `cache.name`, `cache.event`, and `cache.duration_ns`.
**Operation-complete events** carry `cache.name`, `cache.operation`,
`cache.duration_ns`, and `cache.coalesced`.

Use [`telemetry::attributes`][__link23] constants to filter and match events in a
custom `tracing_subscriber::Layer`:

```rust
use cachet::telemetry::attributes;

// Filter by tracing target prefix
let filter = tracing_subscriber::filter::Targets::new()
    .with_target(attributes::TARGET, tracing::Level::DEBUG);

// Match specific events in a Visit impl
if event_value == attributes::EVENT_HIT { /* cache hit */ }
```

See the `telemetry_subscriber` example for a complete demonstration.

#### Event types

|Level|Events|
|-----|------|
|ERROR|`cache.get_error`, `cache.insert_error`, `cache.invalidate_error`, `cache.clear_error`|
|INFO|`cache.expired`, `cache.refresh_miss`, `cache.inserted`, `cache.insert_rejected`, `cache.invalidated`, `cache.eviction`|
|DEBUG|`cache.hit`, `cache.miss`, `cache.refresh_hit`, `cache.cleared`|

### Event handler callback API

Register a [`CacheEventHandler`][__link24] via
`.event_handler(handler)` on the cache builder to receive typed
[`CacheTierEvent`][__link25] and
[`CacheOperationEvent`][__link26] callbacks.
Events carry a `request_id` for correlating tier outcomes with their parent
operation. Works independently of the `logs` feature.

See the `telemetry_accumulator` example for a DashMap-based accumulation pattern.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/cachet">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbJYL19q8vzBMb_QI_68rGifAb8ItjnxSNMDYbUffk0aZ7s_1hZIiCaGJ5dGVzYnVmZTAuNi4wgmZjYWNoZXRlMC44LjCCbWNhY2hldF9tZW1vcnllMC40LjCCbmNhY2hldF9zZXJ2aWNlZTAuMi44gmtjYWNoZXRfdGllcmUwLjIuNoJkdGlja2UwLjQuMIJndHJhY2luZ2YwLjEuNDSCaXVuaWZsaWdodGUwLjMuMA
 [__link0]: https://docs.rs/cachet/0.8.0/cachet/?search=TimeToRefresh
 [__link1]: https://crates.io/crates/uniflight/0.3.0
 [__link10]: https://docs.rs/cachet_tier/0.2.6/cachet_tier/?search=CacheTier
 [__link11]: https://docs.rs/cachet/0.8.0/cachet/?search=InsertPolicy
 [__link12]: https://docs.rs/cachet/0.8.0/cachet/?search=TimeToRefresh
 [__link13]: https://docs.rs/cachet_tier/0.2.6/cachet_tier/?search=Error
 [__link14]: https://crates.io/crates/cachet_tier/0.2.6
 [__link15]: https://crates.io/crates/cachet_memory/0.4.0
 [__link16]: https://docs.rs/moka
 [__link17]: https://crates.io/crates/cachet_service/0.2.8
 [__link18]: https://docs.rs/cachet/0.8.0/cachet/?search=telemetry::attributes
 [__link19]: https://docs.rs/bytesbuf/0.6.0/bytesbuf/?search=BytesView
 [__link2]: https://docs.rs/cachet/0.8.0/cachet/?search=CacheBuilder::stampede_protection
 [__link20]: https://github.com/microsoft/SymCrypt
 [__link21]: https://crates.io/crates/symcrypt
 [__link22]: https://crates.io/crates/tracing/0.1.44
 [__link23]: https://docs.rs/cachet/0.8.0/cachet/?search=telemetry::attributes
 [__link24]: https://docs.rs/cachet/0.8.0/cachet/?search=telemetry::handler::CacheEventHandler
 [__link25]: https://docs.rs/cachet/0.8.0/cachet/?search=telemetry::handler::CacheTierEvent
 [__link26]: https://docs.rs/cachet/0.8.0/cachet/?search=telemetry::handler::CacheOperationEvent
 [__link3]: https://docs.rs/cachet_tier/0.2.6/cachet_tier/?search=CacheTier
 [__link4]: https://docs.rs/cachet_tier/0.2.6/cachet_tier/?search=DynamicCache
 [__link5]: https://docs.rs/cachet/0.8.0/cachet/?search=InsertPolicy
 [__link6]: https://docs.rs/tick/0.4.0/tick/?search=Clock
 [__link7]: https://docs.rs/cachet/0.8.0/cachet/?search=Cache
 [__link8]: https://docs.rs/cachet/0.8.0/cachet/?search=CacheBuilder
 [__link9]: https://docs.rs/cachet_tier/0.2.6/cachet_tier/?search=CacheEntry
