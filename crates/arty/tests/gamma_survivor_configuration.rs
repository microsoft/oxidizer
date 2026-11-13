// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime builder diagnostics without starting workers or clocks.

#![cfg(feature = "rt")]

use arty::runtime::Runtime;

#[test]
fn gamma_survivor_default_stack_size_is_exactly_two_mebibytes() {
    let output = format!("{:?}", Runtime::builder());
    let (_, fields) = output.split_once("stack_size: ").unwrap();
    let stack_size = fields.split([',', '}']).next().unwrap().trim();
    assert_eq!(stack_size, "2097152", "{output}");
}
