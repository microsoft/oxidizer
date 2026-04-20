<div align="center">
 <img src="./logo.png" alt="Cachet Tier Logo" width="96">

# Cachet Tier

[![crate.io](https://img.shields.io/crates/v/cachet_tier.svg)](https://crates.io/crates/cachet_tier)
[![docs.rs](https://docs.rs/cachet_tier/badge.svg)](https://docs.rs/cachet_tier)
[![MSRV](https://img.shields.io/crates/msrv/cachet_tier)](https://crates.io/crates/cachet_tier)
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
[`CacheTier`][__link3] for your storage backend, then use `cachet` to add telemetry, TTL,
multi-tier fallback, and other features on top.

## Implementing a Cache Tier

Implement all required methods of [`CacheTier`][__link4]:

```rust
use std::collections::HashMap;
use std::sync::RwLock;

use cachet_tier::{CacheEntry, CacheTier, Error};

struct SimpleCache<K, V>(RwLock<HashMap<K, CacheEntry<V>>>);

impl<K, V> CacheTier<K, V> for SimpleCache<K, V>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        Ok(self.0.read().unwrap().get(key).cloned())
    }

    async fn insert(&self, key: K, entry: CacheEntry<V>) -> Result<(), Error> {
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

[`DynamicCache`][__link5] wraps any `CacheTier` in a type-erased container. This is useful
for multi-tier caches with heterogeneous storage backends.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/cachet_tier">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG0hRqDfWg1oDG5BT1ZI-3omTG5WE4GB0Mg57G-G4ebzGeSk5YWSBgmtjYWNoZXRfdGllcmUwLjIuMA
 [__link0]: https://docs.rs/cachet_tier/0.2.0/cachet_tier/?search=CacheTier
 [__link1]: https://docs.rs/cachet_tier/0.2.0/cachet_tier/?search=CacheEntry
 [__link2]: https://docs.rs/cachet_tier/0.2.0/cachet_tier/?search=Error
 [__link3]: https://docs.rs/cachet_tier/0.2.0/cachet_tier/?search=CacheTier
 [__link4]: https://docs.rs/cachet_tier/0.2.0/cachet_tier/?search=CacheTier
 [__link5]: https://docs.rs/cachet_tier/0.2.0/cachet_tier/?search=DynamicCache
