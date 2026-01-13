// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
