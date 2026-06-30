// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Direct runtime test of `#[fundle::newtype]`.
//!
//! The macro's only other use is the trybuild compile-test in `tests/proc`,
//! which merely checks that the generated code compiles. This test exercises
//! the generated `From`/`Deref`/`DerefMut` implementations at runtime to verify
//! they behave correctly.

#![allow(missing_docs, reason = "Unit tests")]

#[fundle::newtype]
struct Name(String);

#[test]
fn newtype_from_deref_and_deref_mut() {
    // `From<T: AsRef<String>>` (a `Box<String>` is the simplest std type that
    // satisfies `AsRef<String>`).
    let name = Name::from(Box::new(String::from("hello")));

    // `Deref<Target = String>`
    assert_eq!(name.len(), 5);
    assert_eq!(&*name, "hello");

    // `DerefMut`
    let mut name = name;
    name.push('!');
    assert_eq!(&*name, "hello!");
}
