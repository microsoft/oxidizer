// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Procedural macros to support the [`fundle`](https://docs.rs/fundle) crate. See `fundle` for more information.
//!
//! # Macros
//!
//! ## `#[bundle]`
//!
//! Transforms structs into type-safe builders with dependency injection support.
//!
//! ```rust,ignore
//! #[fundle::bundle]
//! pub struct AppState {
//!    logger: Logger,
//!    database: Database,
//! }
//! ```
//!
//! Generates builder methods and a select macro for dependency access.
//!
//! ## `#[deps]`
//!
//! Creates dependency parameter structs with automatic `From<T>` implementations.
//!
//! ```rust,ignore
//! #[fundle::deps]
//! pub struct ServiceDeps {
//!     logger: Logger,
//!     database: Database,
//! }
//! ```
//!
//! Generates `From<T>` where `T: AsRef<Logger> + AsRef<Database>`.
//!
//! ## `#[newtype]`
//!
//! Creates newtype wrappers with automatic trait implementations.
//!
//! ```rust,ignore
//! #[newtype]
//! pub struct DatabaseLogger(Logger);
//! ```
//!
//! Generates `Clone`, `From<T: AsRef<Logger>>`, `Deref`, and `DerefMut`.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fundle_macros/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fundle_macros/favicon.ico")]

use proc_macro::TokenStream;

/// Define an 'export' DI container.
///
/// The `bundle` macro transforms a struct into a builder pattern where each field must be
/// explicitly provided before the struct can be built. It provides type-state tracking to
/// ensure all required fields are set at compile time.
///
/// # Generated Methods
///
/// For each field `foo` of type `T`, the following methods are generated:
/// - `foo(|builder| -> T)` - Set the field using a closure
/// - `foo_async(|builder| async -> T)` - Async setter
/// - `foo_try(|builder| -> Result<T, E>)` - Fallible setter
/// - `foo_try_async(|builder| async -> Result<T, E>)` - Async fallible setter
///
/// # Select Macro
///
/// Inside setter closures, use the generated `StructName!(select(builder) => Type(field), ...)`
/// macro to access previously set fields by type.
///
/// # Example
///
/// ```rust,ignore
/// # use fundle_proc as fundle;
/// # struct Database;
/// # impl Database { fn connect(_: &str) -> Database { Database } }
/// # struct Logger;
/// # impl Logger { fn new() -> Logger { Logger } }
/// # struct Config;
/// # impl Config { fn new<T>(t: T) -> Config { Config } }
/// #[fundle::bundle]
/// pub struct AppState {
///     database: Database,
///     config: Config,
///     logger_1: Logger,
///     logger_2: Logger,
/// }
///
/// // Usage
/// let app = AppState::builder()
///     .logger(|_| Logger::new())
///     .database(|_| Database::connect("db://localhost"))
///     .config(|x| {
///         let with_logger = AppState!(select(x) => Logger(logger_1));
///         Config::new(with_logger)
///     })
///     .build();
/// ```
///
/// # Forward Attribute
///
/// Use `#[forward]` on fields to forward their `AsRef` implementations to the main struct:
///
/// ```rust,ignore
/// # use fundle_proc as fundle;
/// # pub mod db {
/// #     pub struct Connection;
/// # }
/// # struct Database;
/// #     impl AsRef<db::Connection> for Database {
/// #         fn as_ref(&self) -> &db::Connection { self }
/// #     }
/// # struct Logger;
/// #[fundle::bundle]
/// pub struct AppState {
///     #[forward(db::Connection)]
///     database: Database,  // AppState will impl AsRef<Connection> if Database does
///     logger: Logger,
/// }
/// ```
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn bundle(attr: TokenStream, item: TokenStream) -> TokenStream {
    fundle_macros_impl::bundle(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Define 'import' DI dependencies.
///
/// The `deps` macro generates a struct that can be automatically constructed from any type
/// that implements `AsRef<FieldType>` for each of the struct's field types. This is useful
/// for creating dependency parameter objects that can be constructed from builder states
/// or other aggregate types.
///
/// # Example
///
/// ```rust
/// # use fundle_macros as fundle;
/// # #[derive(Clone)]
/// # struct Config {}
/// # #[derive(Clone)]
/// # struct Database {}
/// # #[derive(Clone)]
/// # struct Logger {}
/// # #[derive(Clone)]
/// # struct Service {}
/// #[fundle::deps]
/// pub struct ServiceDeps {
///     logger: Logger,
///     database: Database,
///     config: Config,
/// }
///
/// impl Service {
///     fn new(deps: impl Into<ServiceDeps>) -> Service {
///         let deps = deps.into();
/// #       Self {}
///     }
/// }
/// ```
/// # Generated Implementation
///
/// For a struct with fields of types `T1`, `T2`, etc., generates:
/// ```rust,ignore
/// impl<T> From<T> for ServiceDeps where T: AsRef<T1> + AsRef<T2> + ...
/// {
///     fn from(value: T) -> Self {
///         Self {
///             field1: value.as_ref().clone(),
///             field2: value.as_ref().clone(),
///             // ...
///         }
///     }
/// }
/// ```
///
/// # Requirements
///
/// - All field types must implement `Clone`
/// - The source type must implement `AsRef<T>` for each field type
/// - Only works with structs that have named fields
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn deps(attr: TokenStream, item: TokenStream) -> TokenStream {
    fundle_macros_impl::deps(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Newtype wrappers to resolve multiple import dependencies.
///
/// The `newtype` macro transforms a tuple struct with a single field into a newtype with
/// automatic implementations for common traits. This is useful for creating type-safe
/// wrappers around existing types while maintaining easy conversion and access patterns.
///
/// # Example
/// ```rust
/// # use fundle_macros as fundle;
/// # #[derive(Clone)]
/// # pub struct Logger;
/// #[fundle::newtype]
/// pub struct DatabaseLogger(Logger);
///
/// #[fundle::newtype]
/// pub struct FileLogger(Logger);
///
/// #[fundle::deps]
/// pub struct ServiceDeps {
///     db_logger: DatabaseLogger,
///     file_logger: FileLogger,
/// }
/// ```
/// # Generated Implementations
///
/// For a newtype `Wrapper(Inner)`, the following traits are automatically implemented:
/// - `Clone` - Derived automatically
/// - `From<T>` where `T: AsRef<Inner>` - Convert from any type that can be referenced as Inner
/// - `Deref` and `DerefMut` - Direct access to the wrapped value's methods
///
/// # Requirements
///
/// - Must be applied to tuple structs with exactly one field
/// - The inner type must implement `Clone` (for the From implementation)
/// - Only works with tuple struct syntax: `struct Wrapper(Inner);`
///
/// # Conversion Flexibility
///
/// The `From<T> where T: AsRef<Inner>` implementation allows conversion from:
/// - The inner type directly: `Inner -> Wrapper`
/// - References to the inner type: `&Inner -> Wrapper` (cloned)
/// - Any other type that implements `AsRef<Inner>`
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn newtype(attr: TokenStream, item: TokenStream) -> TokenStream {
    fundle_macros_impl::newtype(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}
