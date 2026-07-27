<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Cachet Service Logo" width="96">

# Cachet Service

[![crate.io](https://img.shields.io/crates/v/cachet_service.svg)](https://crates.io/crates/cachet_service)
[![docs.rs](https://docs.rs/cachet_service/badge.svg)](https://docs.rs/cachet_service)
[![MSRV](https://img.shields.io/crates/msrv/cachet_service)](https://crates.io/crates/cachet_service)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

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


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/cachet_service">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbcrlL8sHnAG4b1ofYj6gT3UEbqnvnufpKEjIbZAmyA7kxTiRhZIKCbmNhY2hldF9zZXJ2aWNlZTAuMi44gmtjYWNoZXRfdGllcmUwLjIuNg
 [__link0]: https://docs.rs/cachet_service/0.2.8/cachet_service/?search=ServiceAdapter
 [__link1]: https://docs.rs/cachet_tier/0.2.6/cachet_tier/?search=CacheTier
 [__link2]: https://docs.rs/cachet_service/0.2.8/cachet_service/?search=ServiceAdapter
