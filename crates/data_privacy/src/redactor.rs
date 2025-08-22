// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::DataClass;

/// Represents types that can redact data.
pub trait Redactor {
    /// Redacts the given value and calls the output function with the redacted value.
    fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn FnMut(&str));

    /// The exact length of the redacted output if it is a constant.
    ///
    /// This can be used as a hint to optimize buffer allocations.
    #[must_use]
    fn exact_len(&self) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common_taxonomy::CommonTaxonomy::Sensitive;

    struct TestRedactor;

    impl Redactor for TestRedactor {
        fn redact(&self, _data_class: &DataClass, value: &str, output: &mut dyn FnMut(&str)) {
            output(&(value.to_string() + "tomato"));
        }
    }

    #[test]
    fn test_exact_len_default_behavior() {
        let redactor = TestRedactor;
        let mut output_buffer = String::new();
        redactor.redact(&Sensitive.data_class(), "test_value", &mut |s| {
            output_buffer.push_str(s);
        });

        assert_eq!(redactor.exact_len(), None);
        assert_eq!(output_buffer, "test_valuetomato");
    }
}
