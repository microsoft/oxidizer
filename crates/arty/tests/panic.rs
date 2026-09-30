// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Root-task panics propagate through the runtime test entry point.

#![cfg(feature = "rt")]
#![cfg(feature = "macros")]

testing_aids::init_tracing!();

use arty::runtime::Builtins;
use arty::test;

// The macro boundary preserves root-task panic payloads, unlike ordinary task joins.
#[test]
#[should_panic(expected = "this is a panic and we expect it to be visible in the test output")]
async fn main(cx: Builtins) {
    assert_eq!(cx.scheduler().spawn(async |_| 42).await.unwrap(), 42);
    panic!("this is a panic and we expect it to be visible in the test output");
}
