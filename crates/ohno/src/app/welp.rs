// Copyright (c) Microsoft Corporation.

//! Macro for constructing `Error` in place.

/// Construct an `Error` in place.
///
/// This macro provides a convenient way to create an `Error` without explicitly
/// calling `Error::new()`. It's similar to `anyhow::anyhow!`.
///
/// The macro accepts:
/// - A string literal: `welp!("error message")`
/// - An error expression: `welp!(MyError::SomeVariant)`
/// - A format string with arguments: `welp!("error: {}", value)`
///
/// # Examples
///
/// ```rust
/// use ohno::app::AppError;
/// use ohno::welp;
///
/// // Create an error from a string literal
/// let error: AppError = welp!("something went wrong");
/// ```
///
/// ```rust
/// use ohno::{welp, bail};
///
/// fn validate(x: i32) -> Result<(), ohno::AppError> {
///     if x < 0 {
///         return Err(welp!("value must be non-negative, got {}", x));
///     }
///     Ok(())
/// }
/// ```
///
/// Creating an error from another error type:
///
/// ```rust
/// use ohno::{welp, bail};
///
/// fn read_file() -> Result<String, ohno::AppError> {
///     std::fs::read_to_string("file.txt")
///         .map_err(|e| welp!(e))
/// }
/// ```
///
/// Using with format arguments:
///
/// ```rust
/// use ohno::{welp, bail};
///
/// let user_id = 42;
/// let error = welp!("failed to process user {}", user_id);
/// assert!(error.to_string().starts_with("failed to process user 42"));
/// ```
#[macro_export]
macro_rules! welp {
    ($msg:literal $(,)?) => {
        $crate::app::AppError::new(format!($msg))
    };
    ($err:ident $(,)?) => {
        $crate::app::AppError::new($err)
    };
    ($err:expr $(,)?) => {
        $crate::app::AppError::new($err)
    };
    ($fmt:expr, $($arg:tt)*) => {
        $crate::app::AppError::new(format!($fmt, $($arg)*))
    };
}
