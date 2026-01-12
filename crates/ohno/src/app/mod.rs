// Copyright (c) Microsoft Corporation.

//! Application-level error handling.
//!
//! `ohno::AppError` provides a simple, ergonomic error type for applications that need
//! flexible error handling without defining custom error types for every error case.
//!
//! This module is similar to `anyhow` but built on top of ohno's error handling
//! infrastructure, providing automatic backtrace capture and error context.
//!
//! # Examples
//!
//! - **Simple Error Type**: `Error` wraps any error implementing `std::error::Error`
//!   ```no_run
//!   use std::io::Error as IoError;
//!   use ohno::app::AppError;
//!
//!   fn connect() -> Result<(), IoError> {
//!       Err(IoError::other("network unreachable"))
//!   }
//!   fn main() -> Result<(), AppError> {
//!       connect()?;
//!       // ...
//!       Ok(())
//!   }
//!   ```
//!
//! - **Automatic Backtraces**: Captures stack traces at error creation time
//!   ```no_run
//!   use ohno::app::AppError;
//!
//!   let err = AppError::new("something failed");
//!   println!("{}", err.backtrace());
//!   ```
//!
//! - **Error Context**: Add contextual information to errors using `error_trace`
//!   ```no_run
//!   use ohno::{app::Result, error_trace, AppError};
//!
//!   #[error_trace("failed to read config")]
//!   fn read_config() -> Result<()> {
//!       Err(AppError::new("disk error"))
//!   }
//!   ```
//!
//! - **Early Returns**: Use `bail!` macro for convenient early returns
//!   ```no_run
//!   use ohno::{app::Result, bail};
//!
//!   fn validate(value: i32) -> Result<()> {
//!       if value < 0 { bail!("invalid input"); }
//!       Ok(())
//!   }
//!   ```
//!
//! - **In-Place Construction**: Use `welp!` macro to construct errors in place
//!   ```no_run
//!   use ohno::{AppError, welp};
//!
//!   let code = 42;
//!   let err = welp!("failed with code {code}");
//!   ```
//!
//! - **Error Chaining**: Walk error chains to find specific error types
//!   ```no_run
//!   use ohno::{AppError, app::OhWell};
//!
//!   let err = AppError::new("wrapper error");
//!   if let Some(io_err) = err.find_source::<std::io::Error>() {
//!       println!("Found IO error: {}", io_err);
//!   }
//!   ```

mod bail;
mod error;
mod ohwell_trait;
pub mod welp;

pub use error::AppError;
pub use ohwell_trait::OhWell;

/// A type alias for `Result<T, ohno::AppError>`.
///
/// This is a convenience alias to simplify function signatures.
/// Instead of writing `Result<T, ohno::AppError>`, you can write `ohno::app::Result<T>`.
///
/// # Examples
///
/// ```rust
/// use ohno::app::Result;
/// use ohno::bail;
///
/// fn read_config() -> Result<String> {
///     let contents = std::fs::read_to_string("config.txt")?;
///     
///     if contents.is_empty() {
///         bail!("config file is empty");
///     }
///     
///     Ok(contents)
/// }
/// ```
pub type Result<T, E = AppError> = std::result::Result<T, E>;
