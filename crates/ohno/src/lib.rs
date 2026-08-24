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
//! When applied to a struct containing an [`OhnoCore`] field, this macro automatically implements [`std::error::Error`], [`std::fmt::Display`], [`std::fmt::Debug`], and [`From`] conversions.
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
//! # How error text is rendered
//!
//! Without a `#[display("...")]` attribute, the text an error renders depends on whether it has a
//! cause — an error or a string handed to `caused_by`.
//!
//! **With a cause**, the cause's message is printed as it stands: no type name, and no
//! `caused by:` line, so the wrapper leaves no trace in the message line. Enrichment and a
//! backtrace, described below, are still written after it.
//! A cause that is an error also stays in the [`source()`](std::error::Error::source) chain, so a
//! caller that walks the chain still finds it.
//!
//! ```rust
//! use std::io;
//!
//! #[ohno::error]
//! pub struct ConfigError;
//!
//! fn read_config() -> Result<String, ConfigError> {
//!     Err(ConfigError::caused_by(io::Error::new(
//!         io::ErrorKind::NotFound,
//!         "no such file: /etc/app.toml",
//!     )))
//! }
//!
//! let error = read_config().unwrap_err();
//!
//! println!("{error}");
//! // Output: no such file: /etc/app.toml
//! # use ohno::ErrorExt;
//! # assert_eq!(error.message(), "no such file: /etc/app.toml");
//! ```
//!
//! A cause given as a string renders the same way, but it does not join the chain: it is a message
//! rather than an error, so `source()` returns `None` for it.
//!
//! Wrapping therefore adds nothing to the text. A wrapper that should say what it was attempting —
//! "failed to load the configuration", say — has to be given a template; see
//! [Overriding error text](#overriding-error-text).
//!
//! **Without a cause**, there is no message to pass through, so the type's own name is printed:
//!
//! ```rust
//! #[ohno::error]
//! pub struct ConfigError;
//!
//! let error = ConfigError::new();
//!
//! println!("{error}");
//! // Output: ConfigError
//! # use ohno::ErrorExt;
//! # assert_eq!(error.message(), "ConfigError");
//! ```
//!
//! That is a symbol, not an explanation, so an error that renders as its own bare name is a sign
//! that it needs either a cause or a template.
//!
//! ## Enrichment and backtraces
//!
//! The message is only the first part of what `Display` writes — with a template and a cause it is
//! already two lines. Each enrichment entry follows it on its own line, marked with `>` and tagged
//! with the place it was added:
//!
//! ```rust
//! use std::io;
//!
//! #[ohno::error]
//! pub struct ConfigError;
//!
//! #[ohno::enrich_err("failed to load the service configuration")]
//! fn read_config() -> Result<String, ConfigError> {
//!     Err(ConfigError::caused_by(io::Error::new(
//!         io::ErrorKind::NotFound,
//!         "no such file: /etc/app.toml",
//!     )))
//! }
//!
//! let error = read_config().unwrap_err();
//!
//! println!("{error}");
//! // Output: no such file: /etc/app.toml
//! //         > failed to load the service configuration (at src/config.rs:6)
//! # assert!(error.to_string().contains("> failed to load the service configuration (at "));
//! ```
//!
//! A captured backtrace comes last for that level, after that level's enrichment:
//!
//! ```text
//! no such file: /etc/app.toml
//! > failed to load the service configuration (at src/config.rs:6)
//!
//! Backtrace:
//!    0: std::backtrace::Backtrace::capture
//!    1: ohno::backtrace::Backtrace::capture
//!    2: ohno::core::OhnoCore::from_source
//!    3: my_app::config::ConfigError::caused_by
//!    4: my_app::config::read_config
//!    ...
//! ```
//!
//! Whether a backtrace is captured at all is the standard library's decision — see its
//! [environment variables](https://doc.rust-lang.org/std/backtrace/index.html#environment-variables).
//! Use [`ErrorExt::message()`](ErrorExt::message) to read the message without this level's own
//! enrichment or backtrace.
//!
//! Every error owns its [`OhnoCore`], and every core renders its own backtrace, so a chain of
//! wrappers that all use the default rendering prints the message once and one backtrace block per
//! level. The levels are written in turn, innermost first — a wrapper's own enrichment and
//! backtrace follow the complete rendering of the level it wraps, so an outer enrichment entry
//! appears *after* the inner level's `Backtrace:` block, not alongside the other enrichment:
//!
//! ```text
//! no such file: /etc/app.toml
//!
//! Backtrace:
//!    ... frames from where ConfigError wrapped the io::Error ...
//!
//! Backtrace:
//!    ... frames from where StartupError wrapped the ConfigError ...
//! ```
//!
//! This follows from each type holding its own core, and is worth knowing before a type is wrapped
//! several layers deep.
//!
//! # Overriding error text
//!
//! The `#[display("...")]` attribute replaces the rendered message with a template of its own,
//! while still printing the cause after it. A cause that is an error also stays in the
//! [`source()`](std::error::Error::source) chain; a cause given as a string is printed the same way
//! but does not join the chain, exactly as under the default rendering.
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
//! // Output: "Failed to read config with path: /etc/config.toml\ncaused by: file not found"
//! # use ohno::ErrorExt;
//! # assert_eq!(error.message(), "Failed to read config with path: /etc/config.toml\ncaused by: file not found");
//! ```
//!
//! The template string supports field interpolation using `{field_name}` syntax. Unlike the
//! default rendering, the cause is never printed on its own: the custom message always leads, and
//! the cause (if any) follows on the next line, after a `caused by:` label. If the error has no
//! cause, only the custom message is displayed — the type name is never used once a template is
//! given.
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
//! name. Neither the `self.` prefix nor the leading-dot form is accepted:
//!
//! | Argument | Accepted |
//! | --- | --- |
//! | `path.display()` | yes |
//! | `self.path.display()` | no, the `self.` prefix is implicit |
//! | `.path.display()` | no, not a valid expression |
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
