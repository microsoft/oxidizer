// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates redacting an **enum** by implementing the formatting traits by hand.
//!
//! The `#[derive(RedactedDebug)]` and `#[derive(RedactedDisplay)]` macros only work on structs;
//! deriving them on an enum produces a compile-time error. To get redaction-aware formatting for an
//! enum, implement [`RedactedDebug`]/[`RedactedDisplay`] directly.
//!
//! This example uses a pure (data-less) enum whose variant itself is sensitive, so each variant's
//! label is routed through the redactor.
//!
//! For deriving the traits on structs, see the `derive_redaction` example.
//!
//! Run it with:
//!
//! ```bash
//! cargo run --example enum_redaction --package data_privacy
//! ```

use std::fmt::Formatter;

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use data_privacy::{RedactedDebug, RedactedDisplay, RedactionEngine, Redactor, taxonomy};

fn main() {
    // An engine that replaces every PII value with a fixed marker, keeping the output deterministic
    // regardless of the underlying value's length.
    let engine = RedactionEngine::builder()
        .add_class_redactor(
            MyTaxonomy::Pii.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::Insert("<redacted>".into())),
        )
        .build();

    let statuses = [MaritalStatus::Single, MaritalStatus::Married, MaritalStatus::Divorced];

    let mut status_debug = String::new();
    let mut status_display = String::new();
    for status in statuses {
        status_debug.clear();
        status_display.clear();
        engine.redacted_debug(&status, &mut status_debug).unwrap();
        engine.redacted_display(&status, &mut status_display).unwrap();
        println!("enum debug: {status_debug:<12} enum display: {status_display}");
    }
}

#[taxonomy(myco)]
enum MyTaxonomy {
    Pii,
}

#[derive(Clone, Copy)]
enum MaritalStatus {
    Single,
    Married,
    Divorced,
}

impl MaritalStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Single => "Single",
            Self::Married => "Married",
            Self::Divorced => "Divorced",
        }
    }
}

impl RedactedDebug for MaritalStatus {
    fn fmt(&self, redactor: &dyn Redactor, f: &mut Formatter<'_>) -> std::fmt::Result {
        redactor.redact(&MyTaxonomy::Pii.data_class(), self.label(), f)
    }
}

impl RedactedDisplay for MaritalStatus {
    fn fmt(&self, redactor: &dyn Redactor, f: &mut Formatter<'_>) -> std::fmt::Result {
        redactor.redact(&MyTaxonomy::Pii.data_class(), self.label(), f)
    }
}
