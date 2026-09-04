// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Downstream compilation tests for the public procedural macros.

#[test]
#[cfg_attr(miri, ignore)]
fn downstream_macro_ui() {
    let tests = trybuild::TestCases::new();
    tests.pass("tests/ui/pass/*.rs");
    tests.compile_fail("tests/ui/fail/*.rs");
}
