<div align="center">
 <img src="./logo.png" alt="Cachelon Tier Logo" width="96">

# Cachelon Tier

[![crate.io](https://img.shields.io/crates/v/cachelon_tier.svg)](https://crates.io/crates/cachelon_tier)
[![docs.rs](https://docs.rs/cachelon_tier/badge.svg)](https://docs.rs/cachelon_tier)
[![MSRV](https://img.shields.io/crates/msrv/cachelon_tier)](https://crates.io/crates/cachelon_tier)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Core cache tier abstractions for building cache backends.

This crate defines the [`CacheTier`][__link0] trait that all cache implementations must satisfy,
along with [`CacheEntry`][__link1] for storing values with metadata and [`Error`][__link2] types for
fallible operations.

## Overview

The cache tier abstraction separates storage concerns from caching features. Implement
[`CacheTier`][__link3] for your storage backend, then use `cachelon` to add telemetry, TTL,
multi-tier fallback, and other features on top.

## Implementing a Cache Tier

Only [`CacheTier::get`][__link4] and [`CacheTier::insert`][__link5] are required. Other methods have
sensible defaults:

```rust
use cachelon_tier::{CacheEntry, CacheTier};
use std::collections::HashMap;
use std::sync::RwLock;

struct SimpleCache<K, V>(RwLock<HashMap<K, CacheEntry<V>>>);

impl<K, V> CacheTier<K, V> for SimpleCache<K, V>
where
    K: Eq + std::hash::Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    async fn get(&self, key: &K) -> Option<CacheEntry<V>> {
        self.0.read().unwrap().get(key).cloned()
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) {
        self.0.write().unwrap().insert(key.clone(), entry);
    }
}
```

## Dynamic Dispatch

Enable the `dynamic-cache` feature for [`DynamicCache`][__link6], which wraps any `CacheTier`
in a type-erased container. This is useful for multi-tier caches with heterogeneous
storage backends.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/cachelon_tier">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEGyPKOgI1l2AfG51wqT28aGS3G-Q4nmtuKu0UG3A0auuR4tkCYWSBgm1jYWNoZWxvbl90aWVyZTAuMS4w
 [__link0]: https://docs.rs/cachelon_tier/0.1.0/cachelon_tier/?search=CacheTier
 [__link1]: https://docs.rs/cachelon_tier/0.1.0/cachelon_tier/?search=CacheEntry
 [__link2]: https://docs.rs/cachelon_tier/0.1.0/cachelon_tier/?search=error::Error
 [__link3]: https://docs.rs/cachelon_tier/0.1.0/cachelon_tier/?search=CacheTier
 [__link4]: https://docs.rs/cachelon_tier/0.1.0/cachelon_tier/?search=CacheTier::get
 [__link5]: https://docs.rs/cachelon_tier/0.1.0/cachelon_tier/?search=CacheTier::insert
 [__link6]: https://docs.rs/cachelon_tier/0.1.0/cachelon_tier/?search=DynamicCache
