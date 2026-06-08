// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates `#[derive(RedactedDebug)]` and `#[derive(RedactedDisplay)]`.
//!
//! It shows three things:
//!
//! 1. How the derive macros generate redaction-aware `Debug`/`Display`-style formatting for structs.
//! 2. How the `#[unredacted]` attribute opts a field out of redaction.
//! 3. How to support an **enum** (which the derive macros intentionally reject) by implementing
//!    [`RedactedDebug`]/[`RedactedDisplay`] by hand.
//!
//! Run it with:
//!
//! ```bash
//! cargo run --example derive_redaction --package data_privacy
//! ```

use std::fmt::Formatter;

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use data_privacy::{RedactedDebug, RedactedDisplay, RedactionEngine, Redactor, classified, taxonomy};


fn main() {
    // An engine that replaces every PII value with a fixed marker (keeping the output deterministic
    // regardless of the underlying value's length), while letting `Public` data pass through
    // unchanged.
    let engine = RedactionEngine::builder()
        .add_class_redactor(
            MyTaxonomy::Pii.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::Insert("<redacted>".into())),
        )
        .add_class_redactor(
            MyTaxonomy::Public.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough),
        )
        .build();

    let contact = Contact {
        display_name: DisplayName("Alice from Accounting".to_string()),
        email: Email("alice@example.com".to_string()),
        phone: PhoneNumber("+1-555-0100".to_string()),
        preferred: true,
    };

    // Derived `RedactedDebug`: struct/field names are printed, `Pii` fields are redacted, the
    // `Public` `display_name` passes through (with `Debug` quotes), and the `#[unredacted]`
    // `preferred` field keeps its standard `Debug` representation.
    let mut debug_out = String::new();
    engine
        .redacted_debug(&contact, &mut debug_out)
        .expect("writing to a String never fails");
    println!("RedactedDebug:   {debug_out}");

    // Derived `RedactedDisplay`: same layout, but field values use `Display`-style formatting.
    let mut display_out = String::new();
    engine
        .redacted_display(&contact, &mut display_out)
        .expect("writing to a String never fails");
    println!("RedactedDisplay: {display_out}");

    // The hand-written enum implementations route each variant's label through the same redactor.
    let statuses = [
        MaritalStatus::Single,
        MaritalStatus::Married,
        MaritalStatus::Divorced,
    ];

    let mut status_debug = String::new();
    let mut status_display = String::new();
    for status in statuses {
        status_debug.clear();
        status_display.clear();
        engine
            .redacted_debug(&status, &mut status_debug)
            .expect("writing to a String never fails");
        engine
            .redacted_display(&status, &mut status_display)
            .expect("writing to a String never fails");
        println!("enum debug: {status_debug:<22} enum display: {status_display}");
    }

    println!("All assertions passed.");
}


// A taxonomy with two data classes. `myco` is the taxonomy name (think "my company"); it
// becomes the namespace recorded in every `DataClass` produced from this enum.
//
// `Pii` marks sensitive data that must be redacted, while `Public` marks data that is safe to emit
// as-is (it still flows through the same classified containers, but the engine is configured to let
// it pass through unchanged).
#[taxonomy(myco)]
enum MyTaxonomy {
    Pii,
    Public,
}

// Classified containers. The `#[classified]` attribute implements `Classified`, `RedactedDebug`,
// and `RedactedDisplay` for each of these automatically, so they can be nested inside the types
// below.
#[classified(MyTaxonomy::Pii)]
struct Email(String);

#[classified(MyTaxonomy::Pii)]
struct PhoneNumber(String);

// A `Public` container: classified, but not sensitive. It demonstrates that the same machinery can
// carry non-sensitive data that the engine deliberately leaves untouched.
#[classified(MyTaxonomy::Public)]
struct DisplayName(String);

// A named struct that derives both redaction-aware formatting traits.
//
// Each non-`#[unredacted]` field is routed through the redactor; `preferred` is shown verbatim.
#[derive(RedactedDebug, RedactedDisplay)]
struct Contact {
    display_name: DisplayName,
    email: Email,
    phone: PhoneNumber,
    #[unredacted]
    preferred: bool,
}

// The derive macros only work on structs. Attempting to derive on an enum produces a
// compile-time error, for example:
//
// ```compile_fail
// #[derive(RedactedDebug)] // error: RedactedDebug can only be derived for structs
// enum MaritalStatus {
//     Single,
//     Married,
//     Divorced,
// }
// ```
//
// To get redacted formatting for an enum, implement the traits by hand. This is a pure (data-less)
// enum whose variant itself is sensitive, so each variant's label is routed through the redactor
// under the `Pii` data class.
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
