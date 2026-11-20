// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Redacted formatting and buffer boundaries for sensitive values.

use core::fmt::Write;

use data_privacy::{DataClass, RedactedDebug, RedactedDisplay, RedactedToString, Redactor, Sensitive};
use insta::assert_snapshot;

struct TagRedactor;

impl Redactor for TagRedactor {
    fn redacts(&self, _data_class: &DataClass) -> bool {
        true
    }

    fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn Write) -> core::fmt::Result {
        write!(output, "<{}/{}:{}>", data_class.taxonomy(), data_class.name(), value)
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_debug_produces_nonempty_output() {
    let sensitive = Sensitive::new("secret", DataClass::new("test", "pii"));
    let redactor: &dyn Redactor = &TagRedactor;
    let result = core::fmt::from_fn(|f| <Sensitive<_> as RedactedDebug>::fmt(&sensitive, redactor, f)).to_string();
    assert_snapshot!(result, @r#"<test/pii:"secret">"#);
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_display_produces_nonempty_output() {
    let sensitive = Sensitive::new("secret", DataClass::new("test", "pii"));
    let redactor: &dyn Redactor = &TagRedactor;
    let result = core::fmt::from_fn(|f| <Sensitive<_> as RedactedDisplay>::fmt(&sensitive, redactor, f)).to_string();
    assert_snapshot!(result, @"<test/pii:secret>");
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_to_string_produces_nonempty_output() {
    let sensitive = Sensitive::new("secret", DataClass::new("test", "pii"));
    let result = sensitive.to_redacted_string(&TagRedactor);
    assert_snapshot!(result, @"<test/pii:secret>");
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_display_at_exact_buffer_boundary() {
    let value = "x".repeat(128);
    let sensitive = Sensitive::new(value, DataClass::new("test", "pii"));
    let result = sensitive.to_redacted_string(&TagRedactor);
    assert_snapshot!(result, @"<test/pii:xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx>");
}

#[test]
#[cfg_attr(miri, ignore)]
fn redacted_debug_at_exact_buffer_boundary() {
    let value = "x".repeat(126);
    let sensitive = Sensitive::new(value, DataClass::new("test", "pii"));
    let redactor: &dyn Redactor = &TagRedactor;
    let result = core::fmt::from_fn(|f| <Sensitive<_> as RedactedDebug>::fmt(&sensitive, redactor, f)).to_string();
    assert_snapshot!(result, @r#"<test/pii:"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx">"#);
}
