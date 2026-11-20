// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Redacted string conversion through caller-defined formatting.

use core::fmt::Write;

use data_privacy_core::{DataClass, RedactedDisplay, RedactedToString, Redactor};
use insta::assert_snapshot;

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
