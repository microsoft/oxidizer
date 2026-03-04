# cachet_tier ![License: MIT](https://img.shields.io/badge/license-MIT-blue) [![cachet_tier on crates.io](https://img.shields.io/crates/v/cachet_tier)](https://crates.io/crates/cachet_tier) [![cachet_tier on docs.rs](https://docs.rs/cachet_tier/badge.svg)](https://docs.rs/cachet_tier) [![Source Code Repository](https://img.shields.io/badge/Code-On%20GitHub-blue?logo=GitHub)](https://github.com/microsoft/oxidizer/tree/main/crates/cachet_tier) ![Rust Version: 1.88.0](https://img.shields.io/badge/rustc-1.88.0-orange.svg)

Core cache tier abstractions for building cache backends.

This crate defines the [`CacheTier`][__link0] trait that all cache implementations must satisfy,
along with [`CacheEntry`][__link1] for storing values with metadata and [`Error`][__link2] types for
fallible operations.

## Overview

The cache tier abstraction separates storage concerns from caching features. Implement
[`CacheTier`][__link3] for your storage backend, then use `cachet` to add telemetry, TTL,
multi-tier fallback, and other features on top.

## Implementing a Cache Tier

Implement all required methods of [`CacheTier`][__link4]:

```rust
use cachet_tier::{CacheEntry, CacheTier, Error};
use std::collections::HashMap;
use std::sync::RwLock;

struct SimpleCache<K, V>(RwLock<HashMap<K, CacheEntry<V>>>);

impl<K, V> CacheTier<K, V> for SimpleCache<K, V>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        Ok(self.0.read().unwrap().get(key).cloned())
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.0.write().unwrap().insert(key.clone(), entry);
        Ok(())
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        self.0.write().unwrap().remove(key);
        Ok(())
    }

    async fn clear(&self) -> Result<(), Error> {
        self.0.write().unwrap().clear();
        Ok(())
    }
}
```

## Dynamic Dispatch

Enable the `dynamic-cache` feature for [`DynamicCache`][__link5], which wraps any `CacheTier`
in a type-erased container. This is useful for multi-tier caches with heterogeneous
storage backends.


 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEG_W_Gn_kaocAGwCcVPfenh7eGy6gYLEwyIe4G6-xw_FwcbpjYXKEG9x_W0gtWIXGG8Hn-Rfz85pbG019zTmIL6Q_G2OZScM_g-oTYWSBgmtjYWNoZXRfdGllcmUwLjEuMA
 [__link0]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=CacheTier
 [__link1]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=CacheEntry
 [__link2]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=Error
 [__link3]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=CacheTier
 [__link4]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=CacheTier
 [__link5]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=DynamicCache
