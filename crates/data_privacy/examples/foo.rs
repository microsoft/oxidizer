// Copyright (c) Microsoft Corporation.

// Changes TODO:
// - classified() impls for wrapper only, but also does derive logic
// - #[derive(RedactedDebug, RedactedDisplay, RedactedToString) ... aka Redacted)] field by field
// - have public / unknown(?) data class for `String` and co -- types that have valid Debug / Display but may contain whatever data
// - introduce data_privacy_macros_impl crate and move testing
// - emit `const` checks for RedactedDebug and `RedactedDisplay` that types actually implement that?
// - create our own formatter for struct pretty printing.

use std::fmt::{Debug, Formatter};
use data_privacy::{Classified, RedactionEngine};
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

struct Foo {
    x: String,
    b: Personal,
}

impl std::fmt::Debug for Foo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Foo")
            .field("x", &self.x)
            .field("b", &self.b)
            .finish()
    }
}

impl RedactedDebug for Foo {
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "Foo {{")?;
        <String as RedactedDebug>::fmt(&self.x, engine, f)?;
        <Personal as RedactedDebug>::fmt(&self.b, engine, f)?;
        write!(f, "}}")?;
        Ok(())
    }
}


impl std::fmt::Display for Foo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Foo {{ x: {}, b: {} }}", self.x, self.b)
    }
}


fn main() {
    let x = Personal("foo".to_string());

    println!("{}", x);
    println!("{:?}", x);
}
