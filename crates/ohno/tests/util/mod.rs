// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Utilities for testing error spans.

/// Asserts that the error contains the expected span.
/// The span should be provided without the `(at file:line)` part.
/// The `(at file:line)` suffix is stripped from each line before comparison.
///
/// # Example
///
/// ```ignore
/// #[error_span("operation failed with value {value}")]
/// fn failing_function(value: i32) -> Result<(), MyError> {
///     Err(MyError::caused_by("base error"))
/// }
///
/// let error = failing_function(42).unwrap_err();
/// assert_span!(error, "operation failed with value 42");
/// ```
#[macro_export]
macro_rules! assert_span {
    ($error:expr, $expected_span:expr) => {{
        let error_display = format!("{}", $error);

        // Strip the (at file:line) suffix from each line
        let re = regex::Regex::new(r" \(at [^:]+:\d+\)$").unwrap();
        let normalized_lines: Vec<String> = error_display.lines().map(|line| re.replace(line, "").to_string()).collect();

        // Expected span without location suffix
        let expected = format!("> {}", $expected_span);

        assert!(
            normalized_lines.iter().any(|line| line == &expected),
            "Expected span not found.\nExpected: {expected}\nActual error:\n{error_display}"
        );
    }};
}
