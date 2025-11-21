// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::error::Error as StdError;

use crate::{OhnoCore, SpanInfo};

/// Base trait for adding error span to error types.
///
/// This trait provides the fundamental error span addition method and is dyn-compatible.
/// It serves as the base for the more ergonomic `ErrorSpanExt` trait.
pub trait ErrorSpan {
    /// Adds error span information to the error.
    ///
    /// This is the core method that other error span methods build upon.
    fn add_error_span(&mut self, span: SpanInfo);
}

/// Extension trait providing ergonomic error span addition methods.
///
/// This trait extends `ErrorSpan` with convenient methods for adding error spans
/// when converting or working with errors. It provides both immediate and
/// lazy evaluation options.
pub trait ErrorSpanExt: ErrorSpan {
    /// Wraps the error with error span.
    #[must_use]
    fn error_span(mut self, span: impl Into<Cow<'static, str>>) -> Self
    where
        Self: Sized,
    {
        self.add_error_span(SpanInfo::new(span));
        self
    }

    /// Wraps the error with detailed error span including file and line information.
    #[must_use]
    fn detailed_error_span(mut self, span: impl Into<Cow<'static, str>>, file: &'static str, line: u32) -> Self
    where
        Self: Sized,
    {
        self.add_error_span(SpanInfo::detailed(span, file, line));
        self
    }

    /// Wraps the error with lazily evaluated error span.
    #[must_use]
    fn with_error_span<F, R>(mut self, f: F) -> Self
    where
        F: FnOnce() -> R,
        R: Into<Cow<'static, str>>,
        Self: Sized,
    {
        self.add_error_span(SpanInfo::new(f()));
        self
    }

    /// Wraps the error with lazily evaluated detailed error span including file and line information.
    #[must_use]
    fn with_detailed_error_span<F, R>(mut self, f: F, file: &'static str, line: u32) -> Self
    where
        F: FnOnce() -> R,
        R: Into<Cow<'static, str>>,
        Self: Sized,
    {
        self.add_error_span(SpanInfo::detailed(f(), file, line));
        self
    }
}

impl ErrorSpan for OhnoCore {
    fn add_error_span(&mut self, span: SpanInfo) {
        self.data.context.push(span);
    }
}

impl<T, E> ErrorSpan for Result<T, E>
where
    E: StdError + ErrorSpan,
{
    fn add_error_span(&mut self, span: SpanInfo) {
        if let Err(e) = self {
            e.add_error_span(span);
        }
    }
}

// Blanket implementation: all types that implement ErrorSpan automatically get ErrorSpanExt
impl<T: ErrorSpan> ErrorSpanExt for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default, ohno::Error)]
    pub struct TestError {
        pub data: OhnoCore,
    }

    #[test]
    fn test_error_span() {
        let mut error = TestError::default();
        error.add_error_span(SpanInfo::new("Test span"));
        assert_eq!(error.data.data.context.len(), 1);
        assert_eq!(error.data.data.context[0].message, "Test span");
        assert!(error.data.data.context[0].location.is_none());

        error.add_error_span(SpanInfo::detailed("Test span", "test.rs", 10));
        assert_eq!(error.data.data.context.len(), 2);
        assert_eq!(error.data.data.context[1].message, "Test span");
        let location = error.data.data.context[1].location.as_ref().unwrap();
        assert_eq!(location.file, "test.rs");
        assert_eq!(location.line, 10);
    }

    #[test]
    fn test_error_span_ext() {
        let error = TestError::default();
        let mut result: Result<(), _> = Err(error);

        result.add_error_span(SpanInfo::new("Immediate span"));

        let err = result.unwrap_err();
        assert_eq!(err.data.data.context.len(), 1);
        assert_eq!(err.data.data.context[0].message, "Immediate span");
        assert!(err.data.data.context[0].location.is_none());

        result = Err(err).detailed_error_span("Detailed span", "test.rs", 20);
        let err = result.unwrap_err();

        assert_eq!(err.data.data.context.len(), 2);
        assert_eq!(err.data.data.context[1].message, "Detailed span");
        let location = err.data.data.context[1].location.as_ref().unwrap();
        assert_eq!(location.file, "test.rs");
        assert_eq!(location.line, 20);
    }
}
