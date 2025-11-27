// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `Unimplemented` error type.

use std::borrow::Cow;

use crate::OhnoCore;

/// Error type for unimplemented functionality.
///
/// This type is designed to replace panicking macros like [`todo!`] and
/// [`unimplemented!`] with a proper error that can be handled gracefully.
///
/// See the documentation for the [`unimplemented_error!`](crate::unimplemented_error!) macro for
/// more details.
///
/// # Examples
///
/// ```
/// use ohno::{Unimplemented, unimplemented_error};
///
/// fn not_ready_yet() -> Result<(), Unimplemented> {
///     unimplemented_error!("this feature is coming soon")
/// }
/// ```
#[derive(crate::Error, Clone)]
#[no_constructors]
#[display("not implemented at {file}:{line}")]
pub struct Unimplemented {
    file: Cow<'static, str>,
    line: usize,
    core: OhnoCore,
}

impl Unimplemented {
    /// Creates a new `Unimplemented` error.
    #[must_use]
    pub fn new(file: Cow<'static, str>, line: usize) -> Self {
        Self {
            file,
            line,
            core: OhnoCore::new(),
        }
    }

    /// Creates a new `Unimplemented` error with a custom message.
    ///
    /// The message provides additional context about why the functionality
    /// is not yet implemented or what needs to be done.
    #[must_use]
    pub fn with_message(message: impl Into<Cow<'static, str>>, file: Cow<'static, str>, line: usize) -> Self {
        Self {
            file,
            line,
            core: OhnoCore::from(message.into()),
        }
    }

    /// Returns the file path where this error was created.
    #[must_use]
    pub fn file(&self) -> &str {
        &self.file
    }

    /// Returns the line number where this error was created.
    #[must_use]
    pub fn line(&self) -> usize {
        self.line
    }
}

/// Returns an [`Unimplemented`] error from the current function.
///
/// This macro is designed to replace panicking macros like [`todo!`] and
/// [`unimplemented!`] with a proper error that can be handled gracefully.
/// It automatically captures the file and line information and returns early
/// with an `Unimplemented` error.
///
/// Unlike the standard panicking macros, this allows your application to:
///
/// - Continue running and handle the error appropriately
/// - Log the error with full context (file, line, message)
/// - Return meaningful error responses to users instead of crashing
/// - Test error paths without triggering panics
///
/// To prevent accidental use of panicking macros, enable these clippy lints:
///
/// ```toml
/// [workspace.lints.clippy]
/// todo = "deny"
/// unimplemented = "deny"
/// ```
///
/// The error can be automatically converted into any error type that implements
/// `From<Unimplemented>`, making it easy to use in functions with different
/// error types.
///
/// # Examples
///
/// Basic usage without a message:
///
/// ```
/// # use ohno::unimplemented_error;
/// fn future_feature() -> Result<String, ohno::Unimplemented> {
///     unimplemented_error!()
/// }
/// assert!(future_feature().is_err());
///
/// With a custom message:
///
/// ```
/// # use `ohno::unimplemented_error`;
/// fn `experimental_api()` -> Result<(), `ohno::Unimplemented`> {
///     `unimplemented_error!("async` runtime support not yet available")
/// }
/// `assert!(experimental_api().is_err())`;
/// ```
///
/// Automatic conversion to custom error types:
///
/// ```should_panic
/// # use ohno::{unimplemented_error, Unimplemented};
/// #[ohno::error]
/// #[from(Unimplemented)]
/// struct AppError;
///
/// fn app_function() -> Result<(), AppError> {
///     unimplemented_error!("feature coming in v2.0")
/// }
/// ```
#[macro_export]
macro_rules! unimplemented_error {
    () => {
        return Err($crate::Unimplemented::new(std::borrow::Cow::Borrowed(file!()), line!() as usize).into())
    };
    ($ex:expr) => {
        return Err($crate::Unimplemented::with_message($ex, std::borrow::Cow::Borrowed(file!()), line!() as usize).into())
    };
}

#[cfg(test)]
mod test {
    use ohno::ErrorExt;

    use super::*;

    #[test]
    fn basic() {
        fn return_err() -> Result<(), Unimplemented> {
            unimplemented_error!()
        }
        let err = return_err().unwrap_err();
        assert!(err.message().starts_with("not implemented at "), "{err}");
    }

    #[test]
    fn with_message() {
        fn return_err() -> Result<(), Unimplemented> {
            unimplemented_error!("custom message")
        }

        let err = return_err().unwrap_err();
        let message = err.message();
        assert!(message.starts_with("not implemented at "), "{message}");
        assert!(message.contains("custom message"), "{message}");
    }

    #[test]
    fn automatic_conversion() {
        #[derive(Debug)]
        struct CustomError(Unimplemented);

        impl From<Unimplemented> for CustomError {
            fn from(err: Unimplemented) -> Self {
                Self(err)
            }
        }

        fn return_custom_err() -> Result<(), CustomError> {
            unimplemented_error!()
        }

        let err = return_custom_err().unwrap_err();
        let message = err.0.message();
        assert!(message.starts_with("not implemented at "), "{message}");
    }
}
