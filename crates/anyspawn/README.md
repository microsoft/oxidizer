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
* **Simple**: Use built-in constructors or provide a closure
* **Flexible**: Works with any async runtime

## Quick Start

### Using Tokio

```rust
use anyspawn::Spawner;

let spawner = Spawner::new_tokio();
let result = spawner.spawn(async { 1 + 1 }).await;
assert_eq!(result, 2);
```

### Custom Runtime

```rust
use anyspawn::Spawner;

let spawner = Spawner::new_custom("threadpool", |fut| {
    std::thread::spawn(move || futures::executor::block_on(fut));
});

// Returns a JoinHandle that can be awaited or dropped
let handle = spawner.spawn(async { 42 });
```

## Thread-Aware Support

`Spawner` implements [`ThreadAware`][__link1] and supports
per-core isolation via [`Spawner::new_thread_aware`][__link2], enabling
contention-free, NUMA-friendly task dispatch. See the
[thread-aware section on `Spawner`][__link3] for
details and examples.

## Features

* `tokio`: Enables the [`Spawner::new_tokio`][__link4] and
  [`Spawner::new_tokio_with_handle`][__link5] constructors


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/anyspawn">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG9pMzxUlg8D9GxZnrkpQT5jmGyYaBtuYnUetGw3uMQ2o2j_5YWSCgmhhbnlzcGF3bmUwLjMuMIJsdGhyZWFkX2F3YXJlZTAuNi4y
 [__link0]: https://docs.rs/anyspawn/0.3.0/anyspawn/?search=Spawner
 [__link1]: https://docs.rs/thread_aware/0.6.2/thread_aware/?search=ThreadAware
 [__link2]: https://docs.rs/anyspawn/0.3.0/anyspawn/?search=Spawner::new_thread_aware
 [__link3]: Spawner#thread-aware-support
 [__link4]: https://docs.rs/anyspawn/0.3.0/anyspawn/?search=Spawner::new_tokio
 [__link5]: https://docs.rs/anyspawn/0.3.0/anyspawn/?search=Spawner::new_tokio_with_handle
