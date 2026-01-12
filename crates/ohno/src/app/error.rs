// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Application-level error type similar to `anyhow::Error`.
//!
//! [`AppError`] provides a simple, ergonomic error type for applications that need
//! flexible error handling without defining custom error types for every error case.
//!
//! # Examples
//!
//! Use [`AppError::new`] to create an error from a string message or any error type:
//!
//! ```rust
//! use ohno::app::AppError;
//!
//! // Create from a string message
//! let error = AppError::new("something went wrong");
//!
//! // Create from any std::error::Error
//! let io_error = AppError::new(std::io::Error::from(std::io::ErrorKind::NotFound));
//! ```
//!
//! Propagate errors using the `?` operator:
//!
//! ```rust
//! use ohno::app::AppError;
//!
//! fn from_io_error(path: &str) -> Result<(), AppError> {
//!    let _ = std::fs::read_to_string(path)?;
//!    Ok(())
//! }
use std::backtrace::Backtrace;
use std::error::Error as StdError;
use std::fmt;

use ohno::{ErrorExt, OhnoCore};

use crate::Enrichable;

/// Inner error type that implements `ohno::Error`.
#[derive(ohno::Error)]
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
    ///
    /// Walks through the error's source chain and returns the first error that matches type `T`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use ohno::app::AppError;
    ///
    /// let err = AppError::new(std::io::Error::from(std::io::ErrorKind::NotFound));
    ///
    /// let found = err.find_source::<std::io::Error>().unwrap();
    /// assert_eq!(found.kind(), std::io::ErrorKind::NotFound);
    /// ```
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

    /// Converts this error into a boxed trait object.
    ///
    /// This is an escape hatch that allows you to compose [`AppError`] with other
    /// error types that expect a [`Box<dyn StdError + Send + Sync + 'static>`](Box). This is
    /// useful when you need to integrate with APIs that don't directly support
    /// [`AppError`] but can work with standard error trait objects.
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
