// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::{Formatter, Result};

use crate::Redactor;

/// Formats the redacted value using the given formatter.
///
/// This trait behaves similarly to the standard library's [`Debug`](core::fmt::Debug) trait, but it produces a redacted
/// representation of the value based on the provided [`Redactor`].
///
/// Types implementing [`Classified`](crate::Classified) usually implement [`RedactedDebug`] as well.
/// Generally speaking, you should just derive an implementation of this trait.
pub trait RedactedDebug {
    /// Performs the formatting.
    ///
    /// # Errors
    ///
    /// This function returns [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    fn fmt(&self, redactor: &dyn Redactor, f: &mut Formatter) -> Result;
}

/// Formats the redacted value using the given formatter.
///
/// This trait behaves similarly to the standard library's [`Display`](std::fmt::Display) trait, but it produces a redacted
/// representation of the value based on the provided [`Redactor`].
///
/// Types implementing [`Classified`](crate::Classified) usually implement [`RedactedDisplay`] as well.
/// Generally speaking, you should just derive an implementation of this trait.
pub trait RedactedDisplay {
    /// Performs the formatting.
    ///
    /// # Errors
    ///
    /// This function returns [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    fn fmt(&self, redactor: &dyn Redactor, f: &mut Formatter) -> Result;
}

/// Converts a type implementing [`RedactedDisplay`] to a redacted string representation.
pub trait RedactedToString {
    /// Converts the value to a redacted string representation.
    fn to_redacted_string(&self, redactor: &dyn Redactor) -> String;
}

impl<T: RedactedDisplay + ?Sized> RedactedToString for T {
    fn to_redacted_string(&self, redactor: &dyn Redactor) -> String {
        core::fmt::from_fn(|f| self.fmt(redactor, f)).to_string()
    }
}
