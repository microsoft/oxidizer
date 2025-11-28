// Copyright (c) Microsoft Corporation.

// Changes TODO:
// x classified() impls for wrapper only, but also does derive logic
// x #[derive(RedactedDebug, RedactedDisplay, RedactedToString) ... aka Redacted)] field by field
// - have public / unknown(?) data class for `String` and co -- types that have valid Debug / Display but may contain whatever data
// - introduce data_privacy_macros_impl crate and move testing
// - emit `const` checks for RedactedDebug and `RedactedDisplay` that types actually implement that?
// - create our own formatter for struct pretty printing.

use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use data_privacy::{RedactedDebug, RedactionEngine};
use data_privacy::RedactedToString;
use data_privacy_macros::{classified, taxonomy};
use std::fmt::Debug;

#[taxonomy(example)]
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum Tax {
    PII,
    OII,
}


#[classified(Tax::PII)]
#[derive(Clone, Eq, PartialEq, Hash)]
struct Personal(String);

#[derive(Debug, RedactedDebug, RedactedToString)]
struct Foo {
    x: String,
    b: Personal,
}

fn main() {
    let engine = RedactionEngine::builder()
        .add_class_redactor(Tax::PII, SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')))
        .add_class_redactor(Tax::OII, SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag),
        )
        .build();

    let x = Personal("foo".to_string());
    let f = Foo {
        x: "x".to_string(),
        b: x.clone(),
    };
    println!("{x}");
    println!("{x:?}");
    println!("{}", <Foo as RedactedToString>::to_string(&f, &engine));
}
