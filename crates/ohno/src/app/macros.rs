// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Macros for the `app` module.

/// Return early with an [`AppError`](crate::AppError).
///
/// The macro accepts:
/// - A string literal: `bail!("error message")`
/// - An error expression: `bail!(MyAppError::new())`
/// - A format string with arguments: `bail!("error: {value}")`
///
/// # Examples
///
/// ```rust
/// use ohno::{AppError, Result};
/// use ohno::bail;
///
/// fn check_value(x: i32) -> Result<()> {
///     if x < 0 {
///         bail!("value must be non-negative, got {x}");
///     }
///     Ok(())
/// }
/// ```
///
/// ```rust
/// use ohno::Result;
/// use ohno::bail;
///
/// fn parse_config(data: &str) -> Result<()> {
///    if data.is_empty() {
///        bail!("config data cannot be empty");
///    }
///    Ok(())
/// }
/// ```
///
/// Bailing with an expression:
///
/// ```rust
/// use ohno::{AppError, Result};
/// use ohno::bail;
///
/// fn read_file(path: &str) -> Result<String> {
///     bail!(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
/// }
/// ```
#[macro_export]
macro_rules! bail {
    ($msg:literal $(,)?) => {
        return Err($crate::AppError::new(format!($msg)))
    };
    ($fmt:expr, $($arg:tt)*) => {
        return Err($crate::AppError::new(format!($fmt, $($arg)*)))
    };
    ($err:ident $(,)?) => {
        return Err($crate::AppError::new($err))
    };
    ($err:expr $(,)?) => {
        return Err($crate::AppError::new($err))
    };
}

/// Construct an [`AppError`](crate::AppError) in place.
///
/// The macro accepts:
/// - A string literal: `app_err!("error message")`
/// - An error expression: `app_err!(MyError::new())`
/// - A format string with arguments: `app_err!("error: {value}")`
///
/// # Examples
///
/// ```rust
/// use ohno::AppError;
/// use ohno::app_err;
///
/// // Create an error from a string literal
/// let error = app_err!("something went wrong");
/// ```
///
/// ```rust
/// use ohno::Result;
/// use ohno::app_err;
///
/// fn validate(x: i32) -> Result<()> {
///     if x < 0 {
///         return Err(app_err!("value must be non-negative, got {x}"));
///     }
///     Ok(())
/// }
/// ```
///
/// Creating an error from another error type:
///
/// ```rust
/// use ohno::Result;
/// use ohno::app_err;
///
/// fn read_file() {
///     let result = std::fs::read_to_string("file.txt").map_err(|e| app_err!(e));
/// }
/// ```
#[macro_export]
macro_rules! app_err {
    ($msg:literal $(,)?) => {
        $crate::AppError::new(format!($msg))
    };
    ($fmt:expr, $($arg:tt)*) => {
        $crate::AppError::new(format!($fmt, $($arg)*))
    };
    ($err:ident $(,)?) => {
        $crate::AppError::new($err)
    };
    ($err:expr $(,)?) => {
        $crate::AppError::new($err)
    };
}
