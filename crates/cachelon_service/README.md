<div align="center">
 <img src="./logo.png" alt="Cachelon Service Logo" width="96">

# Cachelon Service

[![crate.io](https://img.shields.io/crates/v/cachelon_service.svg)](https://crates.io/crates/cachelon_service)
[![docs.rs](https://docs.rs/cachelon_service/badge.svg)](https://docs.rs/cachelon_service)
[![MSRV](https://img.shields.io/crates/msrv/cachelon_service)](https://crates.io/crates/cachelon_service)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Service pattern integration for cache backends.

This crate provides [`ServiceAdapter`][__link0] to convert any `Service<CacheOperation>` into
a [`CacheTier`][__link1], enabling service middleware composition
(retry, timeout, circuit breaker) for cache storage backends.

## Overview

The adapter provides bidirectional integration:

* **Service → Cache**: Use [`ServiceAdapter`][__link2] to wrap services as cache tiers
* **Cache → Service**: The main `cachelon::Cache` implements `Service<CacheOperation>`

## Quick Start

```rust
use cachelon_service::{ServiceAdapter, CacheOperation, CacheResponse};
use layered::Service;

// Any service implementing Service<CacheOperation> can become a cache tier
let service = MyRemoteCacheService::new();
let tier = ServiceAdapter::new(service);

// Now use `tier` as a CacheTier in multi-tier cache hierarchies
```

## Use Cases

* **Remote caches**: Wrap Redis, Memcached, or custom services as cache tiers
* **Middleware composition**: Add retry, timeout, or circuit breaker before caching
* **Unified abstractions**: Use the same service patterns for caching and other I/O


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/cachelon_service">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG5ETXyg7xZtpG4hc6j-sKxqYG2zNbvHenFe-Gzbh2_skureQYWSCgnBjYWNoZWxvbl9zZXJ2aWNlZTAuMS4wgm1jYWNoZWxvbl90aWVyZTAuMS4w
 [__link0]: https://docs.rs/cachelon_service/0.1.0/cachelon_service/?search=adapter::ServiceAdapter
 [__link1]: https://docs.rs/cachelon_tier/0.1.0/cachelon_tier/?search=CacheTier
 [__link2]: https://docs.rs/cachelon_service/0.1.0/cachelon_service/?search=adapter::ServiceAdapter
