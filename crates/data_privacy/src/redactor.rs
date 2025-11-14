// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::DataClass;
use core::fmt::{Result, Write};

/// Represents types that can redact data.
pub trait Redactor {
    /// Redacts the given value and writes the results to the given output sink.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn Write) -> Result;
}

#[cfg(test)]
mod tests {
    use super::*;
    use data_privacy_macros::taxonomy;

    #[taxonomy(test)]
    enum TestTaxonomy {
        Sensitive,
    }

    struct TestRedactor;

    impl Redactor for TestRedactor {
        fn redact(&self, _data_class: &DataClass, value: &str, output: &mut dyn Write) -> Result {
            write!(output, "{value}tomato")
        }
    }

    #[test]
    fn test_exact_len_default_behavior() {
        let redactor = TestRedactor;
        let mut output_buffer = String::new();
        _ = redactor.redact(&TestTaxonomy::Sensitive.data_class(), "test_value", &mut output_buffer);

        assert_eq!(output_buffer, "test_valuetomato");
    }
}
