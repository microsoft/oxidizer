# cachet_memory ![License: MIT](https://img.shields.io/badge/license-MIT-blue) [![cachet_memory on crates.io](https://img.shields.io/crates/v/cachet_memory)](https://crates.io/crates/cachet_memory) [![cachet_memory on docs.rs](https://docs.rs/cachet_memory/badge.svg)](https://docs.rs/cachet_memory) [![Source Code Repository](https://img.shields.io/badge/Code-On%20GitHub-blue?logo=GitHub)](https://github.com/microsoft/oxidizer/tree/main/crates/cachet_memory) ![Rust Version: 1.88.0](https://img.shields.io/badge/rustc-1.88.0-orange.svg)

High-performance in-memory cache backed by moka.

This crate provides [`InMemoryCache`][__link0], a concurrent in-memory cache using the moka
`TinyLFU` eviction algorithm for excellent hit rates. Use [`InMemoryCacheBuilder`][__link1]
to configure capacity, TTL, and TTI without exposing moka types directly.

## Quick Start

```rust
use cachet_memory::InMemoryCacheBuilder;
use cachet_tier::{CacheEntry, CacheTier};
use std::time::Duration;

let cache = InMemoryCacheBuilder::<String, i32>::new()
    .max_capacity(1000)
    .time_to_live(Duration::from_secs(300))
    .build();

cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();
let value = cache.get(&"key".to_string()).await.unwrap();
assert_eq!(*value.unwrap().value(), 42);
```

## Features

* **Capacity limits**: Set maximum entry count with automatic eviction
* **TTL/TTI**: Configure time-to-live and time-to-idle expiration
* **Thread-safe**: Safe for concurrent access from multiple tasks
* **Zero external types**: Builder API avoids exposing moka in your public API


 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEG_W_Gn_kaocAGwCcVPfenh7eGy6gYLEwyIe4G6-xw_FwcbpjYXKEG0LgX5aG1DAPG_3kRJmS0DdtG3eE7ZfO6e51G5nT_6-TpW8FYWSBgm1jYWNoZXRfbWVtb3J5ZTAuMS4w
 [__link0]: https://docs.rs/cachet_memory/0.1.0/cachet_memory/?search=tier::InMemoryCache
 [__link1]: https://docs.rs/cachet_memory/0.1.0/cachet_memory/?search=builder::InMemoryCacheBuilder
