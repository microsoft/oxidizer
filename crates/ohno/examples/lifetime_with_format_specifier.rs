// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates #[`ohno::error`] with lifetime parameters and format specifiers.

#[ohno::error]
#[display("Validation failed for '{input}' with rules: {rules:?}")]
struct ValidationError<'input, 'rules> {
    input: &'input str,
    rules: &'rules [&'rules str],
}

fn validate_input<'a>(input: &'a str, rules: &'a [&'a str]) -> Result<(), ValidationError<'a, 'a>> {
    Err(ValidationError::new(input, rules))
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let rules = ["no_spaces", "min_length_5", "alphanumeric_only"];
    let e = validate_input("bad!", &rules).unwrap_err();
    println!("{e}");
    println!("\nDebug:\n{e:#?}");
}
