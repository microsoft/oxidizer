// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::backtrace::Backtrace;
use std::error::Error as StdError;
use std::fmt;

use ohno::{ErrorExt, OhnoCore};

use crate::Enrichable;

/// Inner error type that implements `ohno::Error`.
#[derive(ohno::Error, Clone)]
struct Inner {
    inner: OhnoCore,
}

/// Application-level error type that wraps any error.
///
/// [`AppError`] is designed for use in applications where you need a simple,
/// catch-all error type.
///
/// This type automatically captures backtraces and provides error context
/// through the underlying [`OhnoCore`].
///
/// # Examples
///
/// - **Generic Error Handling**: Use [`AppError`] as a catch-all error type in your application
///   ```no_run
///   use std::io::Error as IoError;
///   use ohno::AppError;
///
///   fn connect() -> Result<(), IoError> {
///       Err(IoError::other("network unreachable"))
///   }
///   fn main() -> Result<(), AppError> {
///       connect()?;
///       // ...
///       Ok(())
///   }
///   ```
///
/// - **Automatic Backtraces**: Captures stack traces at error creation time
///   ```no_run
///   use ohno::AppError;
///
///   let err = AppError::new("something failed");
///   println!("{}", err.backtrace());
///   ```
///
/// - **Conversion with additional context**: Converts an error into [`AppError`] with additional
///   context using [`IntoAppError`](crate::IntoAppError).
///
///   ```
///   use ohno::{Result, AppError, IntoAppError};
///
///   fn read_config(path: &str) -> Result<()> {
///       let config = std::fs::read_to_string(path).into_app_err("failed to read config")?;
///       // ...
///       Ok(())
///   }
///   ```
///
/// - **Early Returns**: Use [`bail!`](crate::bail) macro for convenient early returns
///   ```no_run
///   use ohno::Result;
///   use ohno::bail;
///
///   fn validate(value: i32) -> Result<()> {
///       if value < 0 {
///           bail!("invalid input");
///       }
///       Ok(())
///   }
///   ```
///
/// - **In-Place Construction**: Use [`app_err!`](crate::app_err) macro to construct errors in place
///   ```
///   use ohno::app_err;
///
///   let code = 42;
///   let err = app_err!("failed with code {code}");
///   ```
///
/// - **Error Chaining**: Walk error chains to find specific error types
///   ```no_run
///   use ohno::AppError;
///
///   fn handle_error(err: &AppError) {
///     if let Some(io_err) = err.find_source::<std::io::Error>() {
///        println!("Found IO error: {io_err}");
///     }
///   }
///   ```
///
/// - **Passing as a reference to [`std::error::Error`]**:
///
///   ```rust
///   use ohno::AppError;
///
///   fn handle_error(err: &dyn std::error::Error) {
///       println!("Error: {err}");
///   }
///
///   let app_error = AppError::new("an error occurred");
///   handle_error(app_error.as_ref());
///   ```
#[derive(Clone)]
pub struct AppError {
    inner: Inner,
}

impl AppError {
    /// Creates a new [`AppError`] from any type that can be converted into an error.
    pub fn new<E>(error: E) -> Self
    where
        E: Into<Box<dyn StdError + Send + Sync>>,
    {
        Self {
            inner: Inner {
                inner: OhnoCore::from(error),
            },
        }
    }

    /// Returns the source error if this error wraps another error.
    #[must_use]
    pub fn source(&self) -> Option<&(dyn StdError + 'static)> {
        StdError::source(&self.inner)
    }

    /// Finds the first source error of the specified type in the error chain.
    #[must_use]
    pub fn find_source<T: StdError + 'static>(&self) -> Option<&T> {
        let mut source = self.source();
        while let Some(err) = source {
            if let Some(target) = err.downcast_ref::<T>() {
                return Some(target);
            }
            source = StdError::source(err);
        }
        None
    }

    /// Returns a reference to the captured backtrace.
    ///
    /// Provides access to the stack trace captured when the error was created.
    ///
    /// # Backtrace Capture
    ///
    /// Controlled by environment variables:
    /// - `RUST_BACKTRACE=1` enables basic backtrace
    /// - `RUST_BACKTRACE=full` enables full backtrace with all frames
    pub fn backtrace(&self) -> &Backtrace {
        self.inner.backtrace()
    }

    /// Returns top-level error message.
    #[must_use]
    pub fn message(&self) -> String {
        self.inner.message()
    }

    /// Converts error into a `Box<dyn std::error::Error + Send + Sync>` that can be used for
    /// interoperability with other error handling code.
    #[must_use]
    pub fn into_std_error(self) -> Box<dyn StdError + Send + Sync + 'static> {
        Box::new(self.inner)
    }
}

impl fmt::Debug for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // it's intentional to provide a better output when `main` function returns Result<T, AppError>
        fmt::Display::fmt(&self.inner, f)
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}
impl<E> From<E> for AppError
where
    E: Into<Box<dyn StdError + Send + Sync>>,
{
    fn from(error: E) -> Self {
        Self::new(error)
    }
}

impl AsRef<dyn StdError + Send + Sync> for AppError {
    fn as_ref(&self) -> &(dyn StdError + Send + Sync + 'static) {
        &self.inner
    }
}

impl Enrichable for AppError {
    fn add_enrichment(&mut self, entry: crate::EnrichmentEntry) {
        self.inner.add_enrichment(entry);
    }
}
