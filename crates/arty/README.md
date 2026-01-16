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

Async runtime abstractions

This crate provides a [`Spawner`][__link0] type that abstracts task spawning across
different async runtimes without generic infection.

## Design Philosophy

* **Concrete type**: No generics needed in your code
* **Simple**: Use built-in constructors or provide a closure
* **Flexible**: Works with any async runtime

## Quick Start

### Using Tokio

```rust
use arty::Spawner;

let spawner = Spawner::tokio();
let result = spawner.spawn(async { 1 + 1 }).await;
assert_eq!(result, 2);
```

### Custom Runtime

```rust
use arty::Spawner;

let spawner = Spawner::custom(|fut| {
    std::thread::spawn(move || futures::executor::block_on(fut));
});

// Returns a JoinHandle that can be awaited or dropped
let handle = spawner.spawn(async { 42 });
```

## Features

* `tokio` (default): Enables the [`Spawner::tokio`][__link1] constructor
* `custom`: Enables the [`Spawner::custom`][__link2] constructor


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEGyjnWqOXl0hfG5JzJZzygN1qG0YJmwqDZUQGGzO95yYeMZDOYWSBgmRhcnR5ZTAuMS4w
 [__link0]: https://docs.rs/arty/0.1.0/arty/?search=Spawner
 [__link1]: https://docs.rs/arty/0.1.0/arty/?search=Spawner::tokio
 [__link2]: https://docs.rs/arty/0.1.0/arty/?search=Spawner::custom
