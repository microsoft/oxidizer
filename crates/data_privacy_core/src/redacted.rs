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
        struct Adapter<'a, T: ?Sized> {
            inner: &'a T,
            redactor: &'a dyn Redactor,
        }

        impl<T: RedactedDisplay + ?Sized> std::fmt::Display for Adapter<'_, T> {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                <T as RedactedDisplay>::fmt(self.inner, self.redactor, f)
            }
        }

        Adapter { inner: self, redactor }.to_string()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::fmt::Write;

    use insta::assert_snapshot;

    use crate::{DataClass, RedactedDisplay, RedactedToString, Redactor};

    /// A simple test redactor that passes through values with a tag.
    struct TagRedactor;

    impl Redactor for TagRedactor {
        fn redacts(&self, _data_class: &DataClass) -> bool {
            true
        }

        fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn Write) -> core::fmt::Result {
            write!(output, "[{}/{}:{}]", data_class.taxonomy(), data_class.name(), value)
        }
    }

    struct TestValue {
        text: &'static str,
    }

    impl RedactedDisplay for TestValue {
        fn fmt(&self, redactor: &dyn Redactor, f: &mut core::fmt::Formatter) -> core::fmt::Result {
            let data_class = DataClass::new("test", "pii");
            redactor.redact(&data_class, self.text, f)
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn to_redacted_string_produces_correct_output() {
        let value = TestValue { text: "secret" };
        let result = value.to_redacted_string(&TagRedactor);
        assert_snapshot!(result, @"[test/pii:secret]");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn to_redacted_string_is_nonempty_for_empty_input() {
        let value = TestValue { text: "" };
        let result = value.to_redacted_string(&TagRedactor);
        assert_snapshot!(result, @"[test/pii:]");
    }
}
