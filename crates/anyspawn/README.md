<div align="center">
 <img src="./logo.png" alt="Anyspawn Logo" width="96">

# Anyspawn

[![crate.io](https://img.shields.io/crates/v/anyspawn.svg)](https://crates.io/crates/anyspawn)
[![docs.rs](https://docs.rs/anyspawn/badge.svg)](https://docs.rs/anyspawn)
[![MSRV](https://img.shields.io/crates/msrv/anyspawn)](https://crates.io/crates/anyspawn)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A generic task spawner compatible with any async runtime.

This crate provides a [`Spawner`][__link0] type that abstracts task spawning across
different async runtimes without generic infection.

## Design Philosophy

* **Concrete type**: No generics needed in your code
* **Simple**: Use built-in constructors or implement [`SpawnCustom`][__link1]
* **Layered**: Compose middleware closures via [`CustomSpawnerBuilder`][__link2]
* **Flexible**: Works with any async runtime

## Quick Start

### Using Tokio

```rust
use anyspawn::Spawner;

let spawner = Spawner::new_tokio();
let result = spawner.spawn(async { 1 + 1 }).await;
assert_eq!(result, 2);
```

## Thread-Aware Support

`Spawner` implements [`ThreadAware`][__link3] and supports
per-core isolation via custom [`SpawnCustom`][__link4] implementations, enabling
contention-free, NUMA-friendly task dispatch.

## Features

* `tokio`: Enables the [`Spawner::new_tokio`][__link5] and
  [`Spawner::new_tokio_with_handle`][__link6] constructors


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/anyspawn">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQblHE7Bl8YSN4bb97k0EOW-rkbZQa-GdodS-cbCkeYjGZgZ-BhZIKCaGFueXNwYXduZTAuNS41gmx0aHJlYWRfYXdhcmVlMC43LjU
 [__link0]: https://docs.rs/anyspawn/0.5.5/anyspawn/?search=Spawner
 [__link1]: https://docs.rs/anyspawn/0.5.5/anyspawn/?search=SpawnCustom
 [__link2]: https://docs.rs/anyspawn/0.5.5/anyspawn/?search=CustomSpawnerBuilder
 [__link3]: https://docs.rs/thread_aware/0.7.5/thread_aware/?search=ThreadAware
 [__link4]: https://docs.rs/anyspawn/0.5.5/anyspawn/?search=SpawnCustom
 [__link5]: https://docs.rs/anyspawn/0.5.5/anyspawn/?search=Spawner::new_tokio
 [__link6]: https://docs.rs/anyspawn/0.5.5/anyspawn/?search=Spawner::new_tokio_with_handle
