// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Write;

use crate::DataClass;

/// Represents types that can apply redaction to classified data.
///
/// This is the primary interface for redaction. Types implementing this trait
/// decide how to handle sensitive data based on its classification.
///
/// Both high-level redaction engines (such as
/// [`RedactionEngine`](https://docs.rs/data_privacy)) and individual redaction
/// strategies (e.g. hash-based or replacement-based redactors) implement this
/// trait. Custom implementations are possible for testing or specialized
/// scenarios.
pub trait Redactor {
    /// Returns whether this redactor would modify output for the given data class.
    ///
    /// Implementations may return `false` when no transformation is applied — for
    /// example, when a strategy operates in pass-through mode, or when redaction has
    /// been explicitly suppressed for the given class.
    fn redacts(&self, data_class: &DataClass) -> bool;

    /// Redacts a string value with an explicit data classification, sending the results to the output sink.
    ///
    /// # Errors
    ///
    /// This function returns [`Err`] if, and only if, writing to the provided output sink
    /// (which implements [`Write`]) returns [`Err`]. String redaction is considered an infallible
    /// operation; this function only returns a [`std::fmt::Result`] because writing to the underlying
    /// sink might fail.
    fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn Write) -> std::fmt::Result;
}
