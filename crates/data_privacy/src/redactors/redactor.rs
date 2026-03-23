// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Write;

use crate::DataClass;

/// Represents types that can redact data.
pub trait Redactor {
    /// Redacts the given value and writes the results to the given output sink.
    ///
    /// # Errors
    ///
    /// This function returns [`Err`] if, and only if, writing to the provided `output` sink (which implements [`Write`]) returns [`Err`]. String redaction is considered
    /// an infallible operation; this function only returns a [`std::fmt::Result`] because writing to the underlying output sink might fail and it must provide a way to propagate
    /// the fact that an error has occurred back up the stack.
    fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn Write) -> std::fmt::Result;
}
