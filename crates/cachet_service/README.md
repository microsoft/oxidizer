# cachet_service ![License: MIT](https://img.shields.io/badge/license-MIT-blue) [![cachet_service on crates.io](https://img.shields.io/crates/v/cachet_service)](https://crates.io/crates/cachet_service) [![cachet_service on docs.rs](https://docs.rs/cachet_service/badge.svg)](https://docs.rs/cachet_service) [![Source Code Repository](https://img.shields.io/badge/Code-On%20GitHub-blue?logo=GitHub)](https://github.com/microsoft/oxidizer/tree/main/crates/cachet_service) ![Rust Version: 1.88.0](https://img.shields.io/badge/rustc-1.88.0-orange.svg)

Service pattern integration for cache backends.

This crate provides [`ServiceAdapter`][__link0] to convert any `Service<CacheOperation>` into
a [`CacheTier`][__link1], enabling service middleware composition
(retry, timeout, circuit breaker) for cache storage backends.

## Overview

The adapter provides bidirectional integration:

* **Service → Cache**: Use [`ServiceAdapter`][__link2] to wrap services as cache tiers
* **Cache → Service**: The main `cachet::Cache` implements `Service<CacheOperation>`

## Quick Start

```rust
// Any Service<CacheOperation> can become a cache tier
let tier = ServiceAdapter::new(my_service);
```

## Use Cases

* **Remote caches**: Wrap Redis, Memcached, or custom services as cache tiers
* **Middleware composition**: Add retry, timeout, or circuit breaker before caching
* **Unified abstractions**: Use the same service patterns for caching and other I/O


 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEG_W_Gn_kaocAGwCcVPfenh7eGy6gYLEwyIe4G6-xw_FwcbpjYXKEG3K5S_LB5wBuG9aH2I-oE91BG6p757n6ShIyG2QJsgO5MU4kYWSCgm5jYWNoZXRfc2VydmljZWUwLjEuMIJrY2FjaGV0X3RpZXJlMC4xLjA
 [__link0]: https://docs.rs/cachet_service/0.1.0/cachet_service/?search=ServiceAdapter
 [__link1]: https://docs.rs/cachet_tier/0.1.0/cachet_tier/?search=CacheTier
 [__link2]: https://docs.rs/cachet_service/0.1.0/cachet_service/?search=ServiceAdapter
