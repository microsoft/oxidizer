// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time safe dependency injection for Rust.
//!
//! # Summary
//!
//! Fundle if a dependency injection system for service libraries. Library authors
//! can simply declare their dependencies, and applications will not compile unless all dependencies
//! are initialized, without application authors having to pass them one-by-one.
//!
//! # Capabilities
//!
//! - **Type-safe builder pattern** - Each field must be set exactly once before building
//! - **Dependency injection** - Fields can access previously set fields during construction
//! - **Automatic `AsRef` implementations** - Generated for unique field types
//! - **Multiple setter variants** - Regular, try (fallible), async, and async-try setters
//!
//! # Quick Start
//!
//! ```rust
//! # #[derive(Clone)]
//! # pub struct Logger {}
//! # #[derive(Clone)]
//! # pub struct Config {}
//! # #[derive(Clone)]
//! # pub struct Database { }
//! # impl Logger { fn new() -> Self { Self {} } }
//! # impl Config { fn new_with_logger(logger: impl AsRef<Logger>) -> Self { Self {} } }
//! # impl Database { fn connect(url: &str, logger: impl AsRef<Logger>) -> Self { Self { } } }
//! #
//! #[fundle::bundle]
//! pub struct AppState {
//!     logger: Logger,
//!     database: Database,
//!     config: Config,
//! }
//!
//! fn main() {
//!     let app = AppState::builder()
//!         .logger(|_| Logger::new())
//!         .config(|x| Config::new_with_logger(x))
//!         .database(|x| Database::connect("postgresql://localhost", x))
//!         .build();
//! }
//! ```
//!
//! # Macros
//!
//! - `#[fundle::bundle]` - Creates type-safe builders with dependency injection
//! - `#[fundle::deps]` - Generates structs that extract dependencies via `AsRef<T>`
//! - `#[fundle::newtype]` - Creates newtype wrappers with automatic trait implementations

#[doc(hidden)]
pub mod exports;

// Re-export proc macros from fundle_proc
pub use fundle_macros::{bundle, deps, newtype};

// Internal helpers. These are used for type state pattern used by the `bundle` macro.
// Specifically, if you do
//
// ```rust
// #[fundle::bundle]
// struct AppState {
//     logger: Logger,
//     config: Config,
// }
// ```
// It will generate an `AppStateBuilder<LOGGER, CONFIG>` and then use `Set` and `NotSet`
// as marker types to govern when `AsRef` and various helpers are implemented.

#[doc(hidden)]
#[derive(Debug)]
pub struct Set;

#[doc(hidden)]
#[derive(Debug)]
pub struct NotSet;
