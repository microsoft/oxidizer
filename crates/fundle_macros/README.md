<div align="center">
  <img src="./logo.png" alt="Fundle Macros Logo" width="128">

# Fundle Macros

[![crate.io](https://img.shields.io/crates/v/fundle_macros.svg)](https://crates.io/crates/fundle_macros)
[![docs.rs](https://docs.rs/fundle_macros/badge.svg)](https://docs.rs/fundle_macros)
[![MSRV](https://img.shields.io/crates/msrv/fundle_macros)](https://crates.io/crates/fundle_macros)
[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

</div>

* [Summary](#summary)
* [Macros](#macros)
  * [`#[bundle]`](#bundle)
  * [`#[deps]`](#deps)
  * [`#[newtype]`](#newtype)

## Summary

<!-- cargo-rdme start -->

Procedural macros to support the [`fundle`](https://docs.rs/fundle) crate. See `fundle` for more information.

## Macros

### `#[bundle]`

Transforms structs into type-safe builders with dependency injection support.

```rust
#[fundle::bundle]
pub struct AppState {
   logger: Logger,
   database: Database,
}
```

Generates builder methods and a select macro for dependency access.

### `#[deps]`

Creates dependency parameter structs with automatic `From<T>` implementations.

```rust
#[fundle::deps]
pub struct ServiceDeps {
    logger: Logger,
    database: Database,
}
```

Generates `From<T>` where `T: AsRef<Logger> + AsRef<Database>`.

### `#[newtype]`

Creates newtype wrappers with automatic trait implementations.

```rust
#[newtype]
pub struct DatabaseLogger(Logger);
```

Generates `Clone`, `From<T: AsRef<Logger>>`, `Deref`, and `DerefMut`.

<!-- cargo-rdme end -->

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
