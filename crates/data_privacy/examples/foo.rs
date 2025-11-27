// Copyright (c) Microsoft Corporation.

// Changes TODO:
// - classified() impls for wrapper only, but also does derive logic
// - #[derive(RedactedDebug, RedactedDisplay, RedactedToString) ... aka Redacted)] field by field
// - have public / unknown(?) data class for `String` and co -- types that have valid Debug / Display but may contain whatever data
// - introduce data_privacy_macros_impl crate and move testing
// - emit `const` checks for RedactedDebug and `RedactedDisplay` that types actually implement that?
// - create our own formatter for struct pretty printing.

use std::fmt::{Debug, Formatter};
use data_privacy::{Classified, RedactedToString, RedactionEngine, RedactionEngineBuilder, SimpleRedactor, SimpleRedactorMode};
use data_privacy_macros::{classified, taxonomy};
use data_privacy::RedactedDebug;

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
    let engine = RedactionEngineBuilder::new()
        .add_class_redactor(
            &Tax::PII.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')),
        )
        .add_class_redactor(
            &Tax::OII.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag),
        )
        .build();

    let x = Personal("foo".to_string());
    let f = Foo {
        x: "x".to_string(),
        b: x.clone(),
    };
    println!("{}", x);
    println!("{:?}", x);
    println!("{}", <Foo as RedactedToString>::to_string(&f, &engine));
}
