// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time safety tests for URI template parameter types.

use trybuild::TestCases;

#[test]
#[cfg_attr(miri, ignore)]
fn compile_fail_tests() {
    let t = TestCases::new();
    t.compile_fail("tests/ui/string_in_restricted_position.rs");
}
