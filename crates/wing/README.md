<div align="center">
 <img src="./logo.png" alt="Wing Logo" width="96">

# Wing

[![crate.io](https://img.shields.io/crates/v/wing.svg)](https://crates.io/crates/wing)
[![docs.rs](https://docs.rs/wing/badge.svg)](https://docs.rs/wing)
[![MSRV](https://img.shields.io/crates/msrv/wing)](https://crates.io/crates/wing)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Trait-based async runtime abstraction for spawning tasks.

This crate provides a [`Spawner`][__link0] trait that abstracts task spawning across different async runtimes.
Users can implement `Spawner` for any runtime (Tokio, oxidizer, custom runtimes).

## Design Philosophy

* **Trait-based**: Implement [`Spawner`][__link1] for your runtime
* **Simple**: Just one method to implement
* **Flexible**: Works with any async runtime

## Quick Start

### Using Tokio

```rust
use wing::tokio::TokioSpawner;
use wing::Spawner;

let spawner = TokioSpawner;

// Spawn a fire-and-forget task
spawner.spawn(async {
    println!("Task running!");
});
```

### Custom Implementation

```rust
use wing::Spawner;
use std::future::Future;

#[derive(Clone)]
struct MySpawner;

impl Spawner for MySpawner {
    fn spawn<T>(&self, work: T)
    where
        T: Future<Output = ()> + Send + 'static,
    {
        // Your implementation here
        std::thread::spawn(move || futures::executor::block_on(work));
    }
}
```

## Features

* `tokio` (default): Enables [`tokio::TokioSpawner`][__link2] implementation


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/wing">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEGynk_PiPXAuVG-KP_EnSed44G22eKq6fdB7jG6qsjSxCufmjYWSBgmR3aW5nZTAuMS4w
 [__link0]: https://docs.rs/wing/0.1.0/wing/?search=Spawner
 [__link1]: https://docs.rs/wing/0.1.0/wing/?search=Spawner
 [__link2]: https://docs.rs/wing/0.1.0/wing/?search=tokio::TokioSpawner
