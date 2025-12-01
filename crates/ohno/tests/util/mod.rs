// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Utilities for testing error enrichment.

/// Asserts that the error contains the expected enrichment.
/// The message should be provided without the `(at file:line)` part.
/// The `(at file:line)` suffix is stripped from each line before comparison.
///
/// # Example
///
/// ```
/// #[enrich_err("operation failed with value {value}")]
/// fn failing_function(value: i32) -> Result<(), MyError> {
///     Err(MyError::caused_by("base error"))
/// }
///
/// let error = failing_function(42).unwrap_err();
/// assert_enrichment!(error, "operation failed with value 42");
/// ```
#[macro_export]
macro_rules! assert_enrichment {
    ($error:expr, $expected_msg:expr) => {{
        let error_display = format!("{}", $error);

        // Strip the (at file:line) suffix from each line
        let re = regex::Regex::new(r" \(at [^:]+:\d+\)$").unwrap();
        let normalized_lines: Vec<String> = error_display.lines().map(|line| re.replace(line, "").to_string()).collect();

        // Expected message without location suffix
        let expected = format!("> {}", $expected_msg);

        assert!(
            normalized_lines.iter().any(|line| line == &expected),
            "Expected message not found.\nExpected: {expected}\nActual error:\n{error_display}"
        );
    }};
}
