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
- [Capabilities](#capabilities)
- [Quick Start](#quick-start)

## Summary

<!-- cargo-rdme start -->

Safe compile-time dependency injection for Rust.

Fundle is a dependency injection system for service libraries that provides compile-time
safety and zero-cost abstractions for managing complex dependency graphs in large
applications.

## What is Dependency Injection?

Dependency injection is an implementation technique for the Inversion of Control design pattern,
where objects receive their dependencies from external sources rather than creating them internally.
This pattern is essential for:

- **Testability**: Replace real implementations with mocks during testing
- **Modularity**: Decouple components from their concrete dependencies
- **Configuration**: Switch implementations based on environment (dev/prod/test)
- **Maintainability**: Change implementations without modifying dependent code

In large enterprise applications, components often depend on many services like databases,
loggers, configuration systems, external APIs, and other business logic components. Managing
these dependencies manually becomes unwieldy as the application grows. Dependency injection
is intended to help.

## Classic Dependency Injection

Classic dependency injection frameworks (like those found in Java/.NET) suffer from several
fundamental issues:

- **Runtime Failures**. Dependencies are resolved at runtime, meaning missing or misconfigured dependencies only
  surface when an application starts (or worse, when specific code paths execute).

- **Virtual Dispatch Overhead**. Traditional DI relies heavily on interfaces and virtual dispatch, introducing performance
  overhead for every method call.

- **Complex Configuration**. Setting up DI containers requires extensive boilerplate and configuration that's often
  error-prone and hard to maintain.

## How Fundle Works

Fundle takes a fundamentally different approach from classic dependency injection frameworks by
using Rust's type system and compile-time guarantees.

- **Compile-Time Safety**. All dependencies must be satisfied at compile time. Missing dependencies result in compilation
  errors, not runtime panics.

- **Zero-Cost Abstraction**. Fundle generates code that compiles down to simple struct field accesses with no virtual
  dispatch. Dependencies are resolved statically, resulting in the same performance as hand-written code. And monomorphization
  ensures no runtime overhead.

- **Dependency Graph Validation**. Fundle automatically validates that dependency graphs are acyclic and that all required
  dependencies are available when constructing each component:
  As applications grow to hundreds of components, Fundle's compile-time validation prevents
  the "integration hell" common in large codebases. New team members can't accidentally
  break the dependency graph.

## Capabilities

- **Type-safe builder pattern** - Each field must be set exactly once before building
- **Dependency injection** - Fields can access previously set fields during construction
- **Automatic `AsRef` implementations** - Generated for unique field types
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

## Name Origin

The name `fundle::bundle` comes from the "Take Your Daughter to Work Day" episode of the American version of The Office.

<!-- cargo-rdme end -->

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
