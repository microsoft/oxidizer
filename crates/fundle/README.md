<div align="center">
 <img src="./logo.png" alt="Fundle Logo" width="128">

# Fundle

[![crate.io](https://img.shields.io/crates/v/fundle.svg)](https://crates.io/crates/fundle)
[![docs.rs](https://docs.rs/fundle/badge.svg)](https://docs.rs/fundle)
[![MSRV](https://img.shields.io/crates/msrv/fundle)](https://crates.io/crates/fundle)
[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

</div>

- [Summary](#summary)
- [Features](#features)
- [Quick Start](#quick-start)
- [Macros](#macros)

## Summary

<!-- cargo-rdme start -->

Dependency injection for people who hate dependency injection.

## Capabilities

- **Type-safe builder pattern** - Each field must be set exactly once before building
- **Dependency injection** - Fields can access previously set fields during construction
- **Automatic AsRef implementations** - Generated for unique field types
- **Multiple setter variants** - Regular, try (fallible), async, and async-try setters

## Quick Start

```rust
#[fundle::bundle]
pub struct AppState {
    logger: Logger,
    database: Database,
    config: Config,
}

fn main() {
    let app = AppState::builder()
        .logger(|_| Logger::new())
        .config(|x| Config::new_with_logger(x))
        .database(|x| Database::connect("postgresql://localhost", x))
        .build();
}
```

## Macros

- `#[fundle::bundle]` - Creates type-safe builders with dependency injection
- `#[fundle::deps]` - Generates structs that extract dependencies via `AsRef<T>`
- `#[fundle::newtype]` - Creates newtype wrappers with automatic trait implementations

<!-- cargo-rdme end -->

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
