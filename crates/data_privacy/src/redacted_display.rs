// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::RedactionEngine;
use core::fmt::{Formatter, Result};

// Trait for types that support redacted display formatting.
pub trait RedactedDisplay {
    /// Formats the redacted value using the given formatter.
    ///
    /// This trait behaves similarly to the standard library's [`std::fmt::Display`] trait, but it produces a redacted
    /// representation of the value based on the provided [`RedactionEngine`].
    ///
    /// Types implementing [`Classified`] usually implement [`RedactedDisplay`] as well.
    /// Generally speaking, you should just derive an implementation of this trait.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter) -> Result;
}
