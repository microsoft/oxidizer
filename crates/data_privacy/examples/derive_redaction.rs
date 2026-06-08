// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates `#[derive(RedactedDebug)]` and `#[derive(RedactedDisplay)]` on structs.
//!
//! It shows:
//!
//! 1. How the derive macros generate redaction-aware `Debug`/`Display`-style formatting.
//! 2. How the `#[unredacted]` attribute opts a field out of redaction.
//! 3. How different data classes (PII vs OII) are handled by the same engine.
//!
//! For redacting an **enum** (which the derive macros reject), see the `enum_redaction` example.
//!
//! Run it with:
//!
//! ```bash
//! cargo run --example derive_redaction --package data_privacy
//! ```

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use data_privacy::{RedactedDebug, RedactedDisplay, RedactionEngine, classified};

use crate::example_taxonomy::ExampleTaxonomy;

#[path = "employees/example_taxonomy.rs"]
mod example_taxonomy;

fn main() {
    let engine = RedactionEngine::builder()
        .add_class_redactor(
            ExampleTaxonomy::PersonallyIdentifiableInformation.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::Insert("<redacted>".into())),
        )
        .add_class_redactor(
            ExampleTaxonomy::OrganizationallyIdentifiableInformation.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough),
        )
        .build();

    let contact = Contact {
        department: Department("Accounting".to_string()),
        email: Email("alice@example.com".to_string()),
        preferred: true,
    };

    let mut debug_out = String::new();
    engine
        .redacted_debug(&contact, &mut debug_out)
        .expect("writing to a String never fails");
    println!("RedactedDebug:   {debug_out}");

    let mut display_out = String::new();
    engine
        .redacted_display(&contact, &mut display_out)
        .expect("writing to a String never fails");
    println!("RedactedDisplay: {display_out}");
}

#[classified(ExampleTaxonomy::PersonallyIdentifiableInformation)]
struct Email(String);

#[classified(ExampleTaxonomy::OrganizationallyIdentifiableInformation)]
struct Department(String);

#[derive(RedactedDebug, RedactedDisplay)]
struct Contact {
    department: Department,
    email: Email,
    #[unredacted]
    preferred: bool,
}
