// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Error types for cache operations.

/// An error from a cache operation.
///
/// This is an opaque error type that can wrap any underlying error from a cache
/// implementation. Use [`std::error::Error::source()`] to access the underlying
/// cause if needed.
///
/// # Example
///
/// ```
/// use cachelon_tier::Error;
///
/// let error = Error::from_message("operation failed");
/// ```
#[ohno::error]
pub struct Error {}

impl Error {
    /// Creates a new error from any type that can be converted to an error.
    ///
    /// This is the public API for creating cache errors from external crates.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon_tier::Error;
    ///
    /// let error = Error::from_message("operation failed");
    /// ```
    pub fn from_message(cause: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::caused_by(cause)
    }
}

/// A specialized [`Result`] type for cache operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_debug_contains_cause_message() {
        let error = Error::caused_by("test error message");
        let debug_str = format!("{error:?}");
        assert!(
            debug_str.contains("test error message"),
            "debug output should contain the cause message, got: {debug_str}"
        );
    }

    #[test]
    fn error_display_contains_cause_message() {
        let error = Error::caused_by("display test");
        let display_str = format!("{error}");
        assert!(
            display_str.contains("display test"),
            "display output should contain the cause message, got: {display_str}"
        );
    }

    #[test]
    fn result_type_alias_propagates_errors() {
        fn returns_err() -> Result<i32> {
            Err(Error::caused_by("expected failure"))
        }

        let err = returns_err().expect_err("should return an error");
        assert!(format!("{err}").contains("expected failure"));
    }
}
