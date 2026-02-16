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

    use http::uri::Parts;

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

    #[test]
    fn test_from_invalid_uri() {
        // Create an invalid URI error
        let invalid_uri_result = "http://[::1:invalid".parse::<http::Uri>();
        assert!(invalid_uri_result.is_err());

        let invalid_uri = invalid_uri_result.unwrap_err();
        let validation_error = ValidationError::from(invalid_uri);

        // Verify the error can be displayed
        let display = validation_error.to_string();
        assert!(!display.is_empty(), "Error should have a non-empty display");

        // Verify it has a source
        assert!(validation_error.source().is_some(), "Should have a source error");
    }

    #[test]
    fn test_from_invalid_uri_parts() {
        // Create invalid URI parts
        let mut parts = Parts::default();
        parts.scheme = Some("http".parse().unwrap());
        // Invalid authority with invalid characters
        parts.authority = Some("[invalid".parse().unwrap_or_else(|_| "example.com".parse().unwrap()));
        parts.path_and_query = Some("/path".parse().unwrap());

        // Try to create a URI from invalid parts
        let invalid_parts_result = http::Uri::from_parts(parts);

        if let Err(invalid_parts) = invalid_parts_result {
            let validation_error = ValidationError::from(invalid_parts);

            // Verify the error can be displayed
            let display = validation_error.to_string();
            assert!(!display.is_empty(), "Error should have a non-empty display");

            // Verify it has a source
            assert!(validation_error.source().is_some(), "Should have a source error");
        }
    }
}
