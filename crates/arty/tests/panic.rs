// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Task panics propagate through the runtime test entry point.

#![cfg(feature = "rt")]
#![cfg(feature = "macros")]

testing_aids::init_tracing!();

use arty::runtime::Builtins;
use arty::test;

// Validate that when the runtime encounters a panic, it is visible as the test output.
#[test]
#[should_panic = "this is a panic and we expect it to be visible in the test output"]
async fn main(cx: Builtins) {
    cx.scheduler()
        .spawn(async move |_| panic!("this is a panic and we expect it to be visible in the test output"))
        .await;
}
