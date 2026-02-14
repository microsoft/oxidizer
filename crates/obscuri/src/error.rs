// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http::uri::{InvalidUri, InvalidUriParts};

/// Represents errors that occur during URI validation.
///
/// This error type is returned when URI parsing or validation fails,
/// typically due to malformed syntax or invalid URI components. It can wrap
/// errors from the `http` crate, such as `InvalidUri` and `InvalidUriParts`,
/// and exposes them via the `source()` method as `http::Error` when available.
#[ohno::error]
#[from(http::Error)]
pub struct ValidationError;

/// `InvalidUri` is a flavor of `http::Error`
impl From<InvalidUri> for ValidationError {
    fn from(err: InvalidUri) -> Self {
        Self::from(http::Error::from(err))
    }
}

/// `InvalidUriParts` is a flavor of `http::Error`
impl From<InvalidUriParts> for ValidationError {
    fn from(err: InvalidUriParts) -> Self {
        Self::from(http::Error::from(err))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::ValidationError;

    #[test]
    fn test_error_display() {
        let error = ValidationError::caused_by("Test validation error");
        let display = error.to_string();
        assert!(display.starts_with("Test validation error"), "Unexpected message: {display}");
    }

    #[test]
    fn test_source() {
        let error = ValidationError::caused_by("Test validation error");
        assert!(error.source().is_none());
    }
}
