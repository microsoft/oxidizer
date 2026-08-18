// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    not(feature = "app-err"),
    expect(rustdoc::broken_intra_doc_links, reason = "AppError is only available with the 'app-err' feature")
)]
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
//! use ohno::{Error, OhnoCore};
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
//! field to the struct and applies `#[derive(Error)]`. This is the simplest way to create error types
//! without manually managing the error infrastructure.
//!
//! The attribute always adds that field and always generates the error representation from it, so
//! no field may be marked with `#[error]`. Remove the marker to keep the field as data, or use
//! `#[derive(Error)]` directly to place the core by hand.
//!
//! A field of type `OhnoCore` may still be declared, and is then treated as data rather than as the
//! error: it is passed to the generated constructors like any other field, appears in the generated
//! `Debug`, and can be referenced from a `#[display(...)]` template — but it is never read for
//! `source()`, the backtrace, or enrichment, which all come from the injected field.
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
//! The `#[display("...")]` attribute customizes the main error message
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
//! Fields of a tuple struct are interpolated by index, using `{0}`, `{1}`, and so on.
//!
//! ## Format Arguments
//!
//! Anything that is not a plain field reference is passed as a positional argument, with
//! `format!`'s placeholder and argument-counting semantics:
//!
//! ```rust
//! use std::path::PathBuf;
//!
//! #[ohno::error]
//! #[display("failed to read config: {}", path.display())]
//! pub struct ConfigError {
//!     pub path: PathBuf,
//! }
//! ```
//!
//! Positional arguments are implicitly scoped to `self`, so a field is referenced by its bare
//! name. Unlike `thiserror`, neither the `self.` prefix nor the leading-dot form is accepted:
//!
//! | Argument | Accepted |
//! | --- | --- |
//! | `path.display()` | yes |
//! | `self.path.display()` | no, the `self.` prefix is implicit |
//! | `.path.display()` | no, not a valid expression |
//!
//! ## Without `#[display]`
//!
//! When no `#[display]` override is given, the rendered message depends on whether the error
//! has a source.
//!
//! **With a source, the source's message is printed verbatim** — no type name, and no
//! `caused by:` line — while [`source()`](std::error::Error::source) still returns the concrete
//! error. This is the direct equivalent of `thiserror`'s `#[error(transparent)]`, and it is what
//! makes the abstract-wrapper shape described in
//! [Modeling Multiple Failure Conditions](#modeling-multiple-failure-conditions) viable.
//!
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! use ohno::assert_error_message;
//!
//! #[ohno::error]
//! pub struct StorageError;
//!
//! let error = StorageError::caused_by("disk is full");
//!
//! // The wrapper contributes no text of its own.
//! assert_error_message!(error, "disk is full");
//! # }
//! # }
//! ```
//!
//! **Without a source, the bare type name is printed.** The two cases are one `#[from]` apart, so
//! a wrapper that is accidentally constructed without a source renders as `Error: StorageError`
//! rather than a message:
//!
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! use ohno::assert_error_message;
//!
//! #[ohno::error]
//! pub struct StorageError;
//!
//! let error = StorageError::new();
//!
//! assert_error_message!(error, "StorageError");
//! # }
//! # }
//! ```
//!
//! Apply [`#[no_constructors]`](#automatic-constructors) to every transparent wrapper so that a
//! wrapper with no source cannot be built at all, rather than relying on review to catch it.
//!
//! One further caveat: because each ohno error carries its own [`OhnoCore`], a chain of
//! transparent wrappers emits one backtrace block per level when backtrace capture is enabled.
//! A two-level wrapper prints two blocks under `RUST_BACKTRACE=1`. This is inherent to
//! per-type cores rather than a property of the transparent shape itself.
//!
//! # Modeling Multiple Failure Conditions
//!
//! `#[derive(Error)]` rejects enums, because [`OhnoCore`] has to live somewhere and a per-variant
//! core would be meaningless. This is the first question most people arriving from `thiserror`
//! have, since its headline pattern is an enum with one variant per failure condition.
//!
//! There are two workable shapes. The deciding question is: **does anything actually branch on
//! the category?**
//!
//! ## Abstract wrapper — nothing branches on the category
//!
//! Prefer this shape. A fieldless error type with `#[from(..)]` over concrete per-condition error
//! types, relying on the transparent passthrough described
//! [above](#without-display) so the wrapper adds no text of its own:
//!
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! use ohno::{ErrorExt as _, assert_error_message};
//!
//! #[ohno::error]
//! pub struct ParseError;
//!
//! #[ohno::error]
//! pub struct TimeoutError;
//!
//! #[ohno::error]
//! #[derive(Default)]
//! #[from(ParseError, TimeoutError)]
//! pub struct RequestError;
//!
//! let error: RequestError = ParseError::caused_by("unexpected token").into();
//!
//! // The wrapper is transparent: the leaf's message is what users see.
//! assert_error_message!(error, "unexpected token");
//!
//! // A caller that does need to discriminate can still recover the concrete type.
//! assert!(error.find_source::<ParseError>().is_some());
//! assert!(error.find_source::<TimeoutError>().is_none());
//! # }
//! # }
//! ```
//!
//! ## Kind field — production code branches on the category
//!
//! One struct carrying a plain kind enum, a `#[display("{kind}")]` override, and a `kind()`
//! accessor:
//!
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "test-util")] {
//! use std::fmt;
//!
//! use ohno::assert_error_message;
//!
//! #[derive(Clone, Copy, Debug, PartialEq, Eq)]
//! pub enum RequestErrorKind {
//!     Parse,
//!     Timeout,
//! }
//!
//! impl fmt::Display for RequestErrorKind {
//!     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//!         match self {
//!             Self::Parse => f.write_str("request could not be parsed"),
//!             Self::Timeout => f.write_str("request timed out"),
//!         }
//!     }
//! }
//!
//! #[ohno::error]
//! #[display("{kind}")]
//! pub struct RequestError {
//!     kind: RequestErrorKind,
//! }
//!
//! impl RequestError {
//!     #[must_use]
//!     pub fn kind(&self) -> RequestErrorKind {
//!         self.kind
//!     }
//! }
//!
//! let error = RequestError::new(RequestErrorKind::Timeout);
//!
//! assert_eq!(error.kind(), RequestErrorKind::Timeout);
//! assert_error_message!(error, "request timed out");
//! # }
//! # }
//! ```
//!
//! Adopting ohno across nine crates, the abstract wrapper was the right shape almost everywhere,
//! and the kind field was used exactly once — in the single case where production code actually
//! branched on the category. Reach for the kind field only when that is true; matching on a
//! category nothing consumes just adds a public enum to maintain.
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
//! //
//! // impl ConfigError {
//! //     pub(crate) fn new(path: impl Into<String>) -> Self { ... }
//! //     pub(crate) fn caused_by(path: impl Into<String>, error: impl Into<Box<dyn Error...>>) -> Self { ... }
//! // }
//!
//! let error = ConfigError::new("/etc/config.toml");
//! let error_with_cause = ConfigError::caused_by("/etc/config.toml", "File not found");
//! ```
//!
//! **The generated constructors are `pub(crate)`, regardless of the visibility of the error type
//! itself.** They are an implementation convenience for the crate that defines the error, not part
//! of its public API, so a `pub struct` error exported from a library cannot be constructed with
//! `new()` or `caused_by()` by a downstream crate. This is deliberate: it keeps the set of ways an
//! error can be built under the control of the crate that owns it, so adding a field is not a
//! breaking change for callers.
//!
//! **Disabling Automatic Constructors:**
//!
//! `#[no_constructors]` disables the generated constructors, leaving the names `new` and
//! `caused_by` free for hand-written versions. It works only with `#[derive(Error)]`, which
//! requires the `OhnoCore` field to be declared explicitly — and that field is the one the
//! hand-written constructor has to initialize:
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
//!         // Custom constructor logic here
//!         Self {
//!             inner_error: OhnoCore::default(),
//!         }
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
//!     std::fs::read_to_string(path).map_err(MyError::caused_by)
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
//!
//! # Error Labeling
//!
//! [`ErrorLabel`] is a low-cardinality string label for errors, intended for use as a metric
//! tag or structured log field. Labels must be chosen from a small, bounded set known at
//! development time to avoid high-cardinality metric series.
//!
//! ```rust
//! use ohno::ErrorLabel;
//!
//! let label: ErrorLabel = ErrorLabel::from_static("timeout");
//! assert_eq!(label, "timeout");
//!
//! let label = ErrorLabel::from_parts(["http", "client", "timeout"]);
//! assert_eq!(label, "http.client.timeout");
//! ```
//!
//! Use [`ErrorLabel::from_error_chain`] to walk an error's [`source`](std::error::Error::source)
//! chain and build a dotted label from recognized errors:
//!
//! ```rust
//! use ohno::ErrorLabel;
//!
//! let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
//! let label = ErrorLabel::from_error_chain(&io_err, |e| {
//!     e.downcast_ref::<std::io::Error>()
//!         .map(|io| ErrorLabel::from(io.kind()))
//! });
//! assert_eq!(label, "connection_refused");
//! ```
//!
//! Types that carry an [`ErrorLabel`] can implement the [`Labeled`] trait to expose it
//! uniformly via [`Labeled::label`].

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
mod error_label;
mod source;

#[cfg(any(feature = "test-util", test))]
pub mod test_util;

pub use core::OhnoCore;

#[cfg(feature = "app-err")]
pub use app::{AppError, IntoAppError};
pub use enrichable::{Enrichable, EnrichableExt};
pub use enrichment_entry::{EnrichmentEntry, Location};
pub use error_ext::ErrorExt;
pub use error_label::{ErrorLabel, Labeled};
pub use ohno_macros::{Error, enrich_err, error};
