// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::RedactionEngine;
use core::fmt::{Formatter, Result};

/// Formats the redacted value using the given formatter.
///
/// This trait behaves similarly to the standard library's [`Debug`](core::fmt::Debug) trait, but it produces a redacted
/// representation of the value based on the provided [`RedactionEngine`].
///
/// Types implementing [`Classified`](crate::Classified) usually implement [`RedactedDebug`] as well.
/// Generally speaking, you should just derive an implementation of this trait.
pub trait RedactedDebug {
    /// Performs the formatting.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter) -> Result;
}

/// Formats the redacted value using the given formatter.
///
/// This trait behaves similarly to the standard library's [`Display`](std::fmt::Display) trait, but it produces a redacted
/// representation of the value based on the provided [`RedactionEngine`].
///
/// Types implementing [`Classified`](crate::Classified) usually implement [`RedactedDisplay`] as well.
/// Generally speaking, you should just derive an implementation of this trait.
pub trait RedactedDisplay {
    /// Performs the formatting.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter) -> Result;
}

/// Converts a type implementing [`RedactedDisplay`] to a redacted string representation.
pub trait RedactedToString {
    /// Converts the value to a redacted string representation.
    fn to_redacted_string(&self, engine: &RedactionEngine) -> String;
}

impl<T: RedactedDisplay + ?Sized> RedactedToString for T {
    fn to_redacted_string(&self, engine: &RedactionEngine) -> String {
        struct Adapter<'a, T: ?Sized> {
            inner: &'a T,
            engine: &'a RedactionEngine,
        }

        impl<T: RedactedDisplay + ?Sized> std::fmt::Display for Adapter<'_, T> {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                <T as RedactedDisplay>::fmt(self.inner, self.engine, f)
            }
        }

        Adapter { inner: self, engine }.to_string()
    }
}
