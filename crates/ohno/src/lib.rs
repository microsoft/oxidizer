// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "app-err"), expect(rustdoc::broken_intra_doc_links))]
#![expect(clippy::doc_markdown, reason = "AppError in header doesn't look good with backticks")]

//! High-quality error handling for Rust.
//!
//! Ohno combines error wrapping, enrichment messages stacking, backtrace capture, and procedural macros
//! into one ergonomic crate for comprehensive error handling.
//!
//! # Key Features
//!
//! - [**`#[derive(Error)]`**](#derive-macro): Derive macro for automatic `std::error::Error`, [`Display`](std::fmt::Display), [`Debug`](std::fmt::Debug) implementations
//! - [**`#[error]`**](#ohnoerror): Attribute macro for creating error types
//! - [**`#[enrich_err("...")]`**](#error-enrichment): Attribute macro for automatic error enrichment with file and line information.
//! - [**`ErrorExt`**](ohno::ErrorExt): Trait that provides additional methods for ohno error types, it's implemented automatically for all ohno error types
//! - [**`OhnoCore`**](OhnoCore): Core error type that wraps source errors, captures backtraces, and holds enrichment entries
//! - [**`AppError`**](AppError): Application-level error type for general application errors
//!
//! # Quick Start
//!
//! ```rust
//! use std::path::{Path, PathBuf};
//!
//! #[ohno::error]
//! pub struct ConfigError(PathBuf);
//!
//! #[ohno::enrich_err("failed to open file {}", path.as_ref().display())]
//! fn open_file(path: impl AsRef<Path>) -> Result<String, ConfigError> {
//!     std::fs::read_to_string(path.as_ref())
//!         .map_err(|e| ConfigError::caused_by(path.as_ref().to_path_buf(), e))
//! }
//! ```
//!
//! # Derive Macro
//!
//! Derive macro for automatically implementing error traits.
//!
//! When applied to a struct or enum containing an [`OhnoCore`] field,
//! this macro automatically implements [`std::error::Error`], [`std::fmt::Display`], [`std::fmt::Debug`], and [`From`] conversions.
//!
//! > **Note**: `From<std::convert::Infallible>` is implemented by default and calls via [`unreachable!`] macro.
//!
//! ```rust
//! use ohno::{OhnoCore, Error};
//!
//! #[derive(Error)]
//! pub struct MyError {
//!     inner_error: OhnoCore,
//! }
//! ```
//!
//! # `ohno::error`
//!
//! The `#[ohno::error]` attribute macro is a convenience wrapper that automatically adds a `OhnoCore`
//! field to your struct and applies `#[derive(Error)]`. This is the simplest way to create error types
//! without manually managing the error infrastructure.
//!
//! ```rust
//! // Simple error without extra fields
//! #[ohno::error]
//! pub struct ParseError;
//!
//! // Error with multiple fields
//! #[ohno::error]
//! pub struct NetworkError {
//!     host: String,
//!     port: u16,
//! }
//! ```
//!
//! # Display Error Override
//!
//! The `#[display("...")]` attribute allows you to customize the main error message
//! while preserving the underlying error as a cause in the error chain.
//!
//! ```rust
//! use std::path::PathBuf;
//!
//! #[ohno::error]
//! #[display("Failed to read config with path: {path}")]
//! pub struct ConfigError {
//!     pub path: String,
//! }
//!
//! // Usage
//! let error = ConfigError::caused_by("/etc/config.toml", "file not found");
//!
//! // Output: "Failed to read config with path: /etc/config.toml\nCaused by:\n\tfile not found"
//! ```
//!
//! The template string supports field interpolation using `{field_name}` syntax. The underlying
//! error (if any) is automatically shown as "Caused by:" in the error chain. If the inner error
//! has no source, only the custom message is displayed.
//!
//! # Automatic Constructors
//!
//! By default, `#[derive(Error)]` automatically generates `new()` and `caused_by()` constructor methods:
//!
//! ```rust
//! #[ohno::error]
//! struct ConfigError {
//!     path: String,
//! }
//!
//! // The derive macro automatically generates:
//! // - ConfigError::new(path: String) -> Self
//! // - ConfigError::caused_by(path: String, error: impl Into<Box<dyn Error...>>) -> Self
//!
//! let error = ConfigError::new("/etc/config.toml");
//! let error_with_cause = ConfigError::caused_by("/etc/config.toml", "File not found");
//! ```
//!
//! **Disabling Automatic Constructors:**
//!
//! Use `#[no_constructors]` to disable automatic generation when you need custom constructors:
//!
//! ```rust
//! use ohno::{Error, OhnoCore};
//!
//! #[derive(Error)]
//! #[no_constructors]
//! struct CustomError {
//!     inner_error: OhnoCore,
//! }
//!
//! impl CustomError {
//!     pub fn new(custom_logic: bool) -> Self {
//!         // Your custom constructor logic here
//!         Self { inner_error: OhnoCore::default() }
//!     }
//! }
//! ```
//!
//! # Automatic From Implementations
//!
//! The `#[from(Type1, Type2, ...)]` attribute automatically generates `From<Type>` implementations
//! for the specified types. Other fields in the struct are defaulted using `Default::default()`.
//!
//! ```rust
//! #[ohno::error]
//! #[derive(Default)]
//! #[from(std::io::Error, std::fmt::Error)]
//! struct MyError {
//!     optional_field: Option<String>,
//!     code: i32,
//! }
//!
//! // This generates:
//! // impl From<std::io::Error> for MyError { ... }
//! // impl From<std::fmt::Error> for MyError { ... }
//!
//! let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
//! let my_err: MyError = io_err.into(); // Works automatically
//! // optional_field = None, code = 0 (defaulted)
//! ```
//!
//! **Note:** Error's fields must implement `Default` when using `#[from]` to ensure they can be properly initialized.
//!
//! # Error Enrichment
//!
//! The [`#[enrich_err("message")]`](enrich_err) attribute macro adds error enrichment with file and line info to function errors.
//!
//! Functions annotated with [`#[enrich_err("message")]`](enrich_err) automatically wrap any returned `Result`. If
//! the function returns an error, the macro injects a message, including file and line information, into the error chain.
//!
//! **Requirements:**
//! - The function must return a type that implements the `map_err` method (such as `Result` or `Poll`)
//! - The error type must implement the [`Enrichable`] trait (automatically implemented for all ohno error types)
//!
//! **Supported syntax patterns:**
//!
//! 1. **Simple string literals:**
//!
//! ```ignore
//! #[enrich_err("failed to process request")]
//! fn process() -> Result<(), MyError> { /* ... */ }
//! ```
//!
//! 2. **Parameter interpolation:**
//!
//! ```ignore
//! #[enrich_err("failed to read file: {path}")]
//! fn read_file(path: &str) -> Result<String, MyError> { /* ... */ }
//! ```
//!
//! 3. **Complex expressions with method calls:**
//!
//! ```ignore
//! use std::path::Path;
//!
//! #[enrich_err("failed to read file: {}", path.display())]
//! fn read_file(path: &Path) -> Result<String, MyError> { /* ... */ }
//! ```
//!
//! 4. **Multiple expressions and calculations:**
//!
//! ```ignore
//! #[enrich_err("processed {} items with total size {} bytes", items.len(), total_size)]
//! fn process_items(items: &[String], total_size: usize) -> Result<(), MyError> { /* ... */ }
//! ```
//!
//! 5. **Mixed parameter interpolation and format expressions:**
//!
//! ```ignore
//! #[enrich_err("user {user} failed operation with {} items", items.len())]
//! fn user_operation(user: &str, items: &[String]) -> Result<(), MyError> { /* ... */ }
//! ```
//!
//! All patterns include file and line information automatically:
//!
//! ```rust
//! #[ohno::error]
//! struct MyError;
//!
//! #[ohno::enrich_err("failed to open file")]
//! fn open_file(path: &str) -> Result<String, MyError> {
//!     std::fs::read_to_string(path)
//!         .map_err(MyError::caused_by)
//! }
//! // Error output will include: "failed to open file (at src/main.rs:42)"
//! ```
//!
//! # AppError
//!
//! For applications that need a simple, catch-all error type, use [`AppError`]. It
//! automatically captures backtraces and can wrap any error type.
//!
//! To avoid accidental usage in libraries, [`AppError`] is only available when the `app-err`
//! feature is enabled.
//!
//! Example usage:
//!
//! ```rust
//! use ohno::AppError;
//!
//! fn process() -> Result<(), AppError> {
//!     std::fs::read_to_string("file.txt")?; // Automatically converts errors
//!     Ok(())
//! }
//! ```

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno/favicon.ico")]

#[doc(hidden)]
extern crate self as ohno;

#[cfg(feature = "app-err")]
mod app;
mod backtrace;
mod core;
mod enrichable;
mod enrichment_entry;
mod error_ext;
mod source;

#[cfg(any(feature = "test-util", test))]
pub mod test_util;

pub use core::OhnoCore;

#[cfg(feature = "app-err")]
pub use app::{AppError, IntoAppError};
pub use enrichable::{Enrichable, EnrichableExt};
pub use enrichment_entry::{EnrichmentEntry, Location};
pub use error_ext::ErrorExt;
pub use ohno_macros::{Error, enrich_err, error};
