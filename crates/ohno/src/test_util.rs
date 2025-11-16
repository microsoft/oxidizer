// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test utilities for the ohno crate.
//!
//! This module is only available when the `test-util` feature is enabled.

/// Assert that an error message matches the expected value, accounting for potential backtraces.
///
/// This macro checks if the error's display representation exactly matches the expected string,
/// or if it starts with the expected string followed by a backtrace section.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "test-util")]
/// # {
/// use ohno::assert_error_message;
///
/// #[derive(ohno::Error)]
/// struct MyError {
///     inner: ohno::OhnoCore,
/// }
///
/// let error = MyError::caused_by("something went wrong");
/// assert_error_message!(error, "something went wrong");
/// # }
/// ```
#[macro_export]
macro_rules! assert_error_message {
    ($error:expr, $expected:expr) => {{
        let error_string = $error.to_string();
        let expected: &str = $expected;

        if error_string == expected {
            // Exact match, success
        } else {
            // Check if it starts with the expected message followed by backtrace
            let starts_with = format!("{expected}\n\nBacktrace:\n");

            assert!(
                error_string.starts_with(&starts_with),
                "Expected error to be '{}' or start with '{}', but got: '{}'",
                expected,
                starts_with,
                error_string
            );
        }
    }};
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use crate::OhnoCore;

    #[derive(crate::Error)]
    struct MyTestError {
        inner: OhnoCore,
    }

    #[test]
    fn test_assert_error_message_exact_match() {
        let error = MyTestError::caused_by("test message");
        assert_error_message!(error, "test message");
    }

    #[test]
    fn test_assert_error_message_with_backtrace() {
        let mut error = MyTestError::caused_by("test message");
        // Force a backtrace (this will be empty in test mode without RUST_BACKTRACE)
        error.inner.data.backtrace = std::backtrace::Backtrace::disabled();
        assert_error_message!(error, "test message");
    }

    #[test]
    #[should_panic(expected = "Expected error to be")]
    fn test_assert_error_message_mismatch() {
        let error = MyTestError::caused_by("actual message");
        assert_error_message!(error, "expected message");
    }
}
