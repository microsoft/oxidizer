// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Output from a caller-defined redactor.

use std::fmt::Write;

use data_privacy::{DataClass, Redactor, taxonomy};

struct TestRedactor;

impl Redactor for TestRedactor {
    fn redacts(&self, _data_class: &DataClass) -> bool {
        true
    }

    fn redact(&self, _data_class: &DataClass, value: &str, output: &mut dyn Write) -> std::fmt::Result {
        write!(output, "{value}tomato")
    }
}

#[taxonomy(test)]
enum TestTaxonomy {
    Sensitive,
}

#[test]
fn test_exact_len_default_behavior() {
    let redactor = TestRedactor;
    let mut output_buffer = String::new();
    _ = redactor.redact(&TestTaxonomy::Sensitive.data_class(), "test_value", &mut output_buffer);

    assert_eq!(output_buffer, "test_valuetomato");
}
