// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates `#[derive(RedactedDebug)]` and `#[derive(RedactedDisplay)]` on structs.
//!
//! It shows:
//!
//! 1. How the derive macros generate redaction-aware `Debug`/`Display`-style formatting.
//! 2. How the `#[unredacted]` attribute opts a field out of redaction.
//! 3. How different data classes (`Pii` vs `Public`) are handled by the same engine.
//!
//! For redacting an **enum** (which the derive macros reject), see the `enum_redaction` example.
//!
//! Run it with:
//!
//! ```bash
//! cargo run --example derive_redaction --package data_privacy
//! ```

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use data_privacy::{RedactedDebug, RedactedDisplay, RedactionEngine, classified, taxonomy};

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

    // Derived `RedactedDisplay`: same layout, but field values use `Display`-style formatting (so
    // the `Public` `display_name` appears without surrounding quotes).
    let mut display_out = String::new();
    engine
        .redacted_display(&contact, &mut display_out)
        .expect("writing to a String never fails");
    println!("RedactedDisplay: {display_out}");
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
// and `RedactedDisplay` for each of these automatically, so they can be nested inside the struct
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
