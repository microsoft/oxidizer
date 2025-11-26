// Copyright (c) Microsoft Corporation.

// Changes TODO:
// - classified() impls for wrapper only, but also does derive logic
// - #[derive(RedactedDebug, RedactedDisplay, RedactedToString) ... aka Redacted)] field by field
// - have public / unknown(?) data class for `String` and co -- types that have valid Debug / Display but may contain whatever data
// - introduce data_privacy_macros_impl crate and move testing
// - emit `const` checks for RedactedDebug and `RedactedDisplay` that types actually implement that?

use data_privacy::Classified;
use data_privacy_macros::{classified, taxonomy};
use data_privacy::RedactedDebug;

#[taxonomy(example)]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum Tax {
    PII,
    OII,
}

#[classified(Tax::PII)]
struct Personal(String);

fn main() {
    let x = Personal("foo".to_string());

    println!("{}", x);
    println!("{:?}", x);
}
