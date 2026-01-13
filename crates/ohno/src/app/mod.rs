// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Application-level error handling.
//!
//! [`AppError`] provides a simple, ergonomic error type for applications that need
//! flexible error handling without defining custom error types for every error case.
//!
//! # Examples
//!
//! - **Generic Error Handling**: Use [`AppError`] as a catch-all error type in your application
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
//! - **Conversion with additional context**: Converts an error into [`AppError`] with additional
//!   context using [`IntoAppError`]
//!
//!   ```
//!   use ohno::app::{Result, AppError, IntoAppError};
//!
//!   fn read_config(path: &str) -> Result<()> {
//!       let config = std::fs::read_to_string(path).into_app_err("failed to read config")?;
//!       // ...
//!       Ok(())
//!   }
//!   ```
//!
//! - **Early Returns**: Use [`bail!`](crate::bail) macro for convenient early returns
//!   ```no_run
//!   use ohno::app::Result;
//!   use ohno::bail;
//!
//!   fn validate(value: i32) -> Result<()> {
//!       if value < 0 {
//!           bail!("invalid input");
//!       }
//!       Ok(())
//!   }
//!   ```
//!
//! - **In-Place Construction**: Use [`app_err!`](crate::app_err) macro to construct errors in place
//!   ```
//!   use ohno::app_err;
//!
//!   let code = 42;
//!   let err = app_err!("failed with code {code}");
//!   ```
//!
//! - **Error Chaining**: Walk error chains to find specific error types
//!   ```no_run
//!   use ohno::app::AppError;
//!
//!   fn handle_error(err: &AppError) {
//!     if let Some(io_err) = err.find_source::<std::io::Error>() {
//!        println!("Found IO error: {io_err}");
//!     }
//!   }
//!   ```
//! 
//! - **Passing as a reference to [`std::error::Error`]**:
//!
//!   ```rust
//!   use ohno::app::AppError;
//!
//!   fn handle_error(err: &dyn std::error::Error) {
//!       println!("Error: {err}");
//!   }
//!
//!   let app_error = AppError::new("an error occurred");
//!   handle_error(app_error.as_ref());
//!   ```

mod error;
mod into_app_err;
mod macros;

pub use error::AppError;
pub use into_app_err::IntoAppError;

/// A type alias for [`Result<T, AppError>`](std::result::Result).
///
/// This is a convenience alias to simplify function signatures.
/// Instead of writing [`Result<T, AppError>`](std::result::Result), you can write [`Result<T>`](Result).
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
