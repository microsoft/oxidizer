// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Output from a caller-defined redactor.

use std::fmt::Write;

use data_privacy::{DataClass, RedactedToString, Redactor, Sensitive, taxonomy};

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
fn sensitive_to_redacted_string_returns_custom_redactor_output() {
    let redactor = TestRedactor;
    let sensitive = Sensitive::new("test_value", TestTaxonomy::Sensitive.data_class());
    let output_buffer = sensitive.to_redacted_string(&redactor);

    assert_eq!(output_buffer, "test_valuetomato");
}
