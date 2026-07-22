// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! A composable, multi-tier caching library with stampede protection, background
//! refresh, and structured telemetry.
//!
//! # Why Multi-Tier Caching?
//!
//! A single cache is a single point of failure and a capacity ceiling. Multi-tier
//! caching layers fast, small caches in front of slower, larger ones:
//!
//!  - **L1 (primary)** - an in-process memory cache: microsecond latency, bounded
//!    capacity, evicts under pressure.
//!  - **L2 (fallback)** - a remote or larger cache: millisecond latency, much larger
//!    capacity, survives process restarts.
//!
//! On a miss in L1, `cachet` transparently queries L2 and optionally *promotes* the
//! value back into L1 so the next request is fast again. The result is lower average
//! latency, reduced load on the backing store, and resilience when either tier is
//! temporarily unavailable.
//!
//! # Why Background Refresh?
//!
//! TTL-based expiration causes a *synchronous* miss every time an entry ages out:
//! the next caller blocks while the value is recomputed. Background refresh
//! (time-to-refresh, TTR) decouples freshness from latency:
//!
//! - While an entry is still within its TTR, all callers receive the cached value
//!   immediately (a "refresh hit").
//! - Once the TTR elapses, the *next* caller still receives the stale value, but a
//!   background task is spawned to pull a fresh value from the fallback tier.
//! - Subsequent callers continue to hit the cache while the refresh happens, so
//!   latency never spikes.
//!
//! Use [`TimeToRefresh`] together with a fallback tier to enable this pattern.
//!
//! # Cache Stampede Protection
//!
//! A **cache stampede** (also called a thundering herd) occurs when many concurrent
//! requests all miss the cache at the same time - for example, after a cold start or
//! after a popular entry expires. Every request independently computes the value,
//! spiking load on the backing store.
//!
//! `cachet` avoids this with *request coalescing* via the [`uniflight`] crate: when
//! stampede protection is enabled, all concurrent requests for the same key are merged
//! so that only one computes the value. The rest wait and share the result, including
//! any error. Enable it with [`CacheBuilder::stampede_protection`].
//!
//! # Flexibility
//!
//! `cachet` is designed to adapt to your infrastructure rather than the other way
//! around:
//!
//! - **Any storage backend** - implement [`CacheTier`] to plug in Redis, Memcached,
//!   a database, or any other store.
//! - **Service middleware** - with the `service` feature, any
//!   `Service<CacheOperation>` becomes a `CacheTier`, so you can compose retry,
//!   timeout, and circuit-breaker middleware around your storage using standard Tower
//!   or `layered` patterns.
//! - **Dynamic dispatch** - the builder type-erases the storage tier into a
//!   [`DynamicCache<K, V>`], so all builders produce the same `Cache<K, V>`
//!   output type regardless of the underlying storage or tier composition.
//! - **Configurable insert policy** - choose whether, and under what conditions,
//!   values are inserted into a tier ([`InsertPolicy`]).
//! - **Clock injection** - all time-based logic (TTL, TTR, timestamps) goes through
//!   a [`tick::Clock`], making caches fully controllable in tests without sleeping.
//!
//! # Why Use This Instead of Moka/Other Caches?
//!
//! Moka (and similar crates) are excellent single-tier in-process caches. `cachet`
//! builds on top of them and adds:
//!
//! | Feature | Moka | `cachet` |
//! |---|---|---|
//! | In-process memory cache | ✅ | ✅ (via `cachet_memory`) |
//! | Multi-tier / fallback | ❌ | ✅ |
//! | Stampede protection | ❌ | ✅ |
//! | Background refresh | ❌ | ✅ |
//! | Service middleware integration | ❌ | ✅ |
//! | Structured telemetry | ❌ | ✅ |
//! | Pluggable storage backends | ❌ | ✅ |
//! | Clock injection for testing | ❌ | ✅ |
//!
//! If you only need a single in-process cache with no telemetry requirements,
//! `moka` directly may be simpler. If you need any of the above, `cachet` is the
//! right choice.
//!
//! # Major Types
//!
//! | Type | Description |
//! |---|---|
//! | [`Cache`] | The user-facing cache. Wraps any `CacheTier` with `get`, `insert`, `invalidate`, `clear`, `get_or_insert`, `try_get_or_insert`, and `optionally_get_or_insert`. |
//! | [`CacheBuilder`] | Builder for `Cache`. Configure storage, TTL, name, telemetry, fallback, insert policy, stampede protection, and background refresh. |
//! | [`CacheEntry<V>`] | A value together with an optional cached-at timestamp and TTL. Returned by all `get` operations. |
//! | [`CacheTier`] | The core trait for storage backends. Implement this to add your own storage. |
//! | [`InsertPolicy`] | Decides whether a value should be inserted into a tier. |
//! | [`TimeToRefresh`] | Configures background refresh: how stale an entry must be before a background task refreshes it. |
//! | [`Error`] | The error type returned by all fallible cache operations. |
//!
//! # How Tiers Compose
//!
//! Tiers are composed at build time using the builder:
//!
//! ```text
//! Cache::builder::<K, V>(clock)
//!     .memory()                          // L1: fast in-process store
//!     .ttl(Duration::from_secs(30))      // entries expire from L1 after 30 s
//!     .fallback(                         // on L1 miss, consult L2
//!         Cache::builder::<K, V>(clock)
//!             .memory()                  // L2: a second in-process store (or a remote service)
//!             .ttl(Duration::from_secs(300))
//!     )
//!     .insert_policy(InsertPolicy::always())  // control when values are inserted into L1
//!     .time_to_refresh(TimeToRefresh::new(Duration::from_secs(20), spawner))  // refresh L1 in background
//!     .build()
//! ```
//!
//! On a `get`:
//! 1. Check L1. If hit and not stale, return immediately.
//! 2. If hit but stale (TTR elapsed), return the stale value *and* spawn a background
//!    task to fetch from L2 and repopulate L1.
//! 3. If miss or expired (TTL elapsed), check L2. If found, optionally promote to L1,
//!    then return.
//! 4. If both miss, return `Ok(None)`.
//!
//! **Note:** expired entries are not automatically removed from storage. The wrapper
//! uses lazy expiration - it returns `None` but leaves cleanup to the storage
//! backend (e.g. moka built-in eviction).
//!
//! TO-DO add an `ExpirationPolicy` that would make this configurable.
//!
//! Invalidation and clear are sent to **all** tiers concurrently.
//!
//! # Companion Crates
//!
//! `cachet` is the main entry point. The ecosystem is split into focused crates:
//!
//! | Crate | Purpose |
//! |---|---|
//! | [`cachet_tier`] | Core `CacheTier` trait, `CacheEntry`, `Error`, and `MockCache` for testing. |
//! | [`cachet_memory`] | In-process memory cache backed by [moka](https://docs.rs/moka) (`TinyLFU` eviction). |
//! | [`cachet_service`] | Adapters between the `CacheTier` trait and the `layered::Service` / Tower service patterns. |
//!
//! You rarely need to depend on companion crates directly - `cachet` re-exports the
//! most commonly used types from all of them.
//!
//! # Cargo Features
//!
//! | Feature | Default | Description |
//! |---|---|---|
//! | `memory` | ✅ | Enables `InMemoryCache` and the `.memory()` builder method via `cachet_memory`. |
//! | `logs` | ❌ | Enables structured `tracing` log events for every cache operation. Subscribe via [`telemetry::attributes`] constants. |
//! | `service` | ❌ | Enables `ServiceAdapter`, `CacheServiceExt`, and `CacheOperation`/`CacheResponse` types for service middleware integration. |
//! | `serialize` | ❌ | Enables `.serialize()` on builders for automatic postcard serialization of keys and values to `BytesView`. |
//! | `encrypt` | ❌ | Enables `.protect_with(protector)` on serialized builders and the `ValueProtector` trait for authenticated value protection with a caller-supplied implementation. |
//! | `test-util` | ❌ | Enables `MockCache`, frozen-clock utilities, and other test helpers. |
//!
//! # Examples
//!
//! ## Basic In-Memory Cache
//!
//! ```no_run
//! use cachet::{Cache, CacheEntry};
//! use tick::Clock;
//! # async {
//!
//! let clock = Clock::new_tokio();
//! let cache: Cache<String, i32> = Cache::builder(clock).memory().build();
//!
//! cache.insert("key".to_string(), CacheEntry::new(42)).await?;
//! let value = cache.get("key").await?;
//! assert_eq!(*value.unwrap().value(), 42);
//! # Ok::<(), cachet::Error>(())
//! # };
//! ```
//!
//! ## Multi-Tier Cache with Fallback
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use cachet::Cache;
//! use tick::Clock;
//! # async {
//!
//! let clock = Clock::new_tokio();
//! let l2 = Cache::builder::<String, String>(clock.clone()).memory();
//!
//! let cache = Cache::builder::<String, String>(clock)
//!     .memory()
//!     .ttl(Duration::from_secs(60))
//!     .fallback(l2)
//!     .build();
//! # };
//! ```
//!
//! ## Serialization Boundary
//!
//! When a fallback tier operates on serialized bytes (e.g., Redis), use `.serialize()`
//! to add a postcard serialization boundary. Keys and values are automatically serialized
//! to [`BytesView`](bytesbuf::BytesView) before reaching the fallback tier, and
//! deserialized on the way back.
//!
//! ```ignore
//! use cachet::{Cache, FallbackPromotionPolicy};
//! use tick::Clock;
//! # async {
//!
//! let clock = Clock::new_tokio();
//! let remote = Cache::builder::<bytesbuf::BytesView, bytesbuf::BytesView>(clock.clone()).memory();
//!
//! let cache = Cache::builder::<String, String>(clock)
//!     .memory()
//!     .serialize()
//!     .fallback(remote)
//!     .promotion_policy(FallbackPromotionPolicy::always())
//!     .build();
//!
//! // Keys and values are String on the outside, BytesView in the fallback tier.
//! cache.insert("key".to_string(), "value".to_string()).await?;
//! # Ok::<(), cachet::Error>(())
//! # };
//! ```
//!
//! ## Encryption Boundary
//!
//! With the `encrypt` feature, chain `.protect_with(protector)` after `.serialize()` to
//! protect values with a caller-supplied `ValueProtector` before they reach the
//! fallback tier. The cachet crate ships only the protection *mechanism* — it has **no
//! cryptographic dependency of its own**, so you plug in a protector backed by whichever
//! approved cryptographic library your project mandates. The protector receives each
//! value's storage key as its context and must bind it, which cryptographically binds
//! every value to its key. (The protect/unprotect contract mirrors OS data-protection
//! APIs such as the Windows DPAPI `CryptProtectData` function.)
//!
//! Only values are protected: keys are left serialized-but-unprotected so they remain
//! deterministic and can be looked up — so do not place secrets or PII in cache keys.
//! A stored value that fails to unprotect (corrupt, truncated, wrong key, tampered, or
//! relocated to a different key) is treated as a cache miss and emits a
//! `cache.unprotect_failed` telemetry event.
//!
//! ```ignore
//! use cachet::Cache;
//! use tick::Clock;
//! # async {
//!
//! let clock = Clock::new_tokio();
//! let remote = Cache::builder::<bytesbuf::BytesView, bytesbuf::BytesView>(clock.clone()).memory();
//!
//! let cache = Cache::builder::<String, String>(clock)
//!     .memory()
//!     .serialize()
//!     .protect_with(my_protector) // any `ValueProtector` implementation
//!     .fallback(remote)
//!     .build();
//!
//! cache.insert("key".to_string(), "value".to_string()).await?;
//! # Ok::<(), cachet::Error>(())
//! # };
//! ```
//!
//! ### Example: a `SymCrypt`-backed AES-256-GCM protector
//!
//! [SymCrypt](https://github.com/microsoft/SymCrypt) is a FIPS-certifiable,
//! SDL-approved cryptographic library. The following `ValueProtector` implementation
//! wraps it using the [`symcrypt`](https://crates.io/crates/symcrypt) crate; it stores
//! each value as `nonce || ciphertext || tag` with a fresh random 96-bit nonce and
//! binds the storage key as associated data. It is shown here as a reference rather
//! than shipped as a compiled feature, because `SymCrypt` requires the native library
//! to be present at build and run time. Add `symcrypt` and `getrandom` to your own
//! crate to use it.
//!
//! ```ignore
//! use bytesbuf::BytesView;
//! use cachet::{DecodeOutcome, Error, ValueProtector};
//! use symcrypt::cipher::BlockCipherType;
//! use symcrypt::gcm::GcmExpandedKey;
//!
//! const NONCE_SIZE: usize = 12;
//! const TAG_SIZE: usize = 16;
//!
//! pub struct Aes256GcmProtector {
//!     key: GcmExpandedKey,
//! }
//!
//! impl Aes256GcmProtector {
//!     pub fn new(key: &[u8; 32]) -> Self {
//!         let key = GcmExpandedKey::new(key, BlockCipherType::AesBlock)
//!             .expect("AES-256-GCM key expansion cannot fail for a valid 32-byte key");
//!         Self { key }
//!     }
//! }
//!
//! impl ValueProtector for Aes256GcmProtector {
//!     fn protect(&self, context: &[u8], plaintext: &BytesView) -> Result<BytesView, Error> {
//!         let mut nonce = [0u8; NONCE_SIZE];
//!         getrandom::fill(&mut nonce).map_err(|e| Error::from_message(format!("nonce: {e}")))?;
//!
//!         // Assemble `nonce || plaintext || tag`, copying the plaintext in once, then
//!         // encrypt the ciphertext region in place and write the tag into the tail.
//!         let plaintext_len = plaintext.len();
//!         let mut result = vec![0u8; NONCE_SIZE + plaintext_len + TAG_SIZE];
//!         result[..NONCE_SIZE].copy_from_slice(&nonce);
//!         let mut offset = NONCE_SIZE;
//!         for (slice, _) in plaintext.slices() {
//!             result[offset..offset + slice.len()].copy_from_slice(slice);
//!             offset += slice.len();
//!         }
//!         let (head, tag) = result.split_at_mut(NONCE_SIZE + plaintext_len);
//!         self.key.encrypt_in_place(&nonce, context, &mut head[NONCE_SIZE..], tag);
//!         Ok(result.into())
//!     }
//!
//!     fn unprotect(&self, context: &[u8], protected: &BytesView) -> Result<DecodeOutcome<BytesView>, Error> {
//!         let bytes = protected.to_vec();
//!         if bytes.len() < NONCE_SIZE + TAG_SIZE {
//!             return Ok(DecodeOutcome::SoftFailure("ciphertext too short"));
//!         }
//!         let (nonce, rest) = bytes.split_at(NONCE_SIZE);
//!         let (body, tag) = rest.split_at(rest.len() - TAG_SIZE);
//!         let nonce: &[u8; NONCE_SIZE] = nonce.try_into().expect("exactly 12 bytes");
//!
//!         let mut buffer = body.to_vec();
//!         match self.key.decrypt_in_place(nonce, context, &mut buffer, tag) {
//!             // Any authentication failure is a soft failure: the entry reads as a miss.
//!             Ok(()) => Ok(DecodeOutcome::Value(buffer.into())),
//!             Err(_) => Ok(DecodeOutcome::SoftFailure("AES-GCM decryption failed")),
//!         }
//!     }
//! }
//! ```
//!
//! Because each protect uses a fresh random 96-bit nonce, rotate the key periodically
//! under extreme write volumes to stay well within the birthday bound.
//!
//! # Telemetry
//!
//! Cachet provides two complementary telemetry channels:
//!
//! ## Tracing events
//!
//! Enable with the `logs` feature and `.enable_logs()` on the cache builder.
//! Each tier outcome and operation completion emits a structured [`tracing`] event.
//!
//! **Tier events** carry `cache.name`, `cache.event`, and `cache.duration_ns`.
//! **Operation-complete events** carry `cache.name`, `cache.operation`,
//! `cache.duration_ns`, and `cache.coalesced`.
//!
//! Use [`telemetry::attributes`] constants to filter and match events in a
//! custom `tracing_subscriber::Layer`:
//!
//! ```ignore
//! use cachet::telemetry::attributes;
//!
//! // Filter by tracing target prefix
//! let filter = tracing_subscriber::filter::Targets::new()
//!     .with_target(attributes::TARGET, tracing::Level::DEBUG);
//!
//! // Match specific events in a Visit impl
//! if event_value == attributes::EVENT_HIT { /* cache hit */ }
//! ```
//!
//! See the `telemetry_subscriber` example for a complete demonstration.
//!
//! ### Event types
//!
//! | Level | Events |
//! |-------|--------|
//! | ERROR | `cache.get_error`, `cache.insert_error`, `cache.invalidate_error`, `cache.clear_error` |
//! | INFO  | `cache.expired`, `cache.refresh_miss`, `cache.inserted`, `cache.insert_rejected`, `cache.invalidated`, `cache.eviction` |
//! | DEBUG | `cache.hit`, `cache.miss`, `cache.refresh_hit`, `cache.cleared` |
//!
//! ## Event handler callback API
//!
//! Register a [`CacheEventHandler`] via
//! `.event_handler(handler)` on the cache builder to receive typed
//! [`CacheTierEvent`] and
//! [`CacheOperationEvent`] callbacks.
//! Events carry a `request_id` for correlating tier outcomes with their parent
//! operation. Works independently of the `logs` feature.
//!
//! See the `telemetry_accumulator` example for a DashMap-based accumulation pattern.

mod builder;
mod cache;
#[cfg(feature = "memory")]
mod eviction;
mod fallback;
mod policy;
mod refresh;
#[cfg(any(feature = "serialize", test))]
mod serialize;
pub mod telemetry;
mod transform;
mod wrapper;

#[cfg(feature = "encrypt")]
#[doc(inline)]
pub use builder::ProtectedTransformBuilder;
#[doc(inline)]
pub use builder::{CacheBuilder, CacheTierBuilder, FallbackBuilder, TransformBuilder};
#[doc(inline)]
pub use cache::{Cache, CacheName};
#[cfg(any(feature = "memory", test))]
#[doc(inline)]
pub use cachet_memory::InMemoryCache;
#[cfg(feature = "service")]
#[doc(inline)]
pub use cachet_service::{CacheOperation, CacheResponse, CacheServiceExt, GetRequest, InsertRequest, InvalidateRequest, ServiceAdapter};
#[doc(inline)]
pub use cachet_tier::DynamicCache;
#[doc(inline)]
pub use cachet_tier::{CacheEntry, CacheTier, Error, Result, SizeError};
#[cfg(any(feature = "test-util", test))]
#[doc(inline)]
pub use cachet_tier::{CacheOp, MockCache};
#[doc(inline)]
pub use policy::InsertPolicy;
#[doc(inline)]
pub use refresh::TimeToRefresh;
#[doc(inline)]
pub use telemetry::handler::{CacheEventHandler, CacheOperationEvent, CacheTierEvent};
#[cfg(all(feature = "encrypt", any(feature = "test-util", test)))]
#[doc(inline)]
pub use transform::MockValueProtector;
#[cfg(feature = "encrypt")]
#[doc(inline)]
pub use transform::ValueProtector;
#[doc(inline)]
pub use transform::{Codec, DecodeOutcome, Encoder, TransformCodec, TransformEncoder, infallible, infallible_owned};

// Installs a silent, always-interested global `tracing` subscriber before any
// unit test in this crate runs. This keeps `tracing` emission paths executing
// deterministically (never poisoned into the "disabled" state) and lets per-test
// thread-local subscribers compose safely. See `docs/tracing-tests.md`.
#[cfg(test)]
testing_aids::init_tracing!();
