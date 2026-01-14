<div align="center">
 <img src="./logo.png" alt="Arty Logo" width="96">

# Arty

[![crate.io](https://img.shields.io/crates/v/arty.svg)](https://crates.io/crates/arty)
[![docs.rs](https://docs.rs/arty/badge.svg)](https://docs.rs/arty)
[![MSRV](https://img.shields.io/crates/msrv/arty)](https://crates.io/crates/arty)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Runtime-agnostic async task spawning.

This crate provides a [`Spawner`][__link0] enum that abstracts task spawning across
different async runtimes without generic infection.

## Design Philosophy

* **Concrete type**: No generics needed in your code
* **Simple**: Use built-in variants or provide a closure
* **Flexible**: Works with any async runtime

## Quick Start

### Using Tokio

```rust
use arty::Spawner;

let spawner = Spawner::Tokio;
spawner.spawn(async {
    println!("Task running!");
});
```

### Custom Runtime

```rust
use arty::Spawner;

let spawner = Spawner::new_custom(|fut| {
    std::thread::spawn(move || futures::executor::block_on(fut));
});

spawner.spawn(async {
    println!("Running on custom runtime!");
});
```

## Features

* `tokio` (default): Enables the [`Spawner::Tokio`][__link1] variant


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEGy8QLYWQ6TvkGzOuS19rYB8RG4HN5vQgcR69G8qNc7VUYU_3YWSBgmRhcnR5ZTAuMS4w
 [__link0]: https://docs.rs/arty/0.1.0/arty/?search=Spawner
 [__link1]: https://docs.rs/arty/0.1.0/arty/?search=Spawner::Tokio
