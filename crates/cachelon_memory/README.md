<div align="center">
 <img src="./logo.png" alt="Cachelon Memory Logo" width="96">

# Cachelon Memory

[![crate.io](https://img.shields.io/crates/v/cachelon_memory.svg)](https://crates.io/crates/cachelon_memory)
[![docs.rs](https://docs.rs/cachelon_memory/badge.svg)](https://docs.rs/cachelon_memory)
[![MSRV](https://img.shields.io/crates/msrv/cachelon_memory)](https://crates.io/crates/cachelon_memory)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

High-performance in-memory cache backed by moka.

This crate provides [`InMemoryCache`][__link0], a concurrent in-memory cache using the moka
`TinyLFU` eviction algorithm for excellent hit rates. Use [`InMemoryCacheBuilder`][__link1]
to configure capacity, TTL, and TTI without exposing moka types directly.

## Quick Start

```rust
use cachelon_memory::InMemoryCacheBuilder;
use cachelon_tier::{CacheEntry, CacheTier};
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


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/cachelon_memory">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEGxfGy3mv6QMSG0GS5SKyO_EiG2PLzpZ-SFFoG3HW2zkIYzCDYWSBgm9jYWNoZWxvbl9tZW1vcnllMC4xLjA
 [__link0]: https://docs.rs/cachelon_memory/0.1.0/cachelon_memory/?search=tier::InMemoryCache
 [__link1]: https://docs.rs/cachelon_memory/0.1.0/cachelon_memory/?search=builder::InMemoryCacheBuilder
