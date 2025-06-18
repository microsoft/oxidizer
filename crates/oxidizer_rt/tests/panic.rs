// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.
#![cfg(feature = "macros")]

use std::panic::{catch_unwind, resume_unwind};

use oxidizer_rt::BasicThreadState;

#[oxidizer_rt::test]
// Note that should_panic only checks the panic output of the main thread. Even though we also
// get a panic message from the worker thread, the test infrastructure does not see it. You need
// to explicitly check the message in test code if you want to see what panic occurs in async code.
// See test_panic_explicit for an example of how to do that.
#[should_panic(expected = "sender dropped without setting result")]
async fn test_panic(cx: BasicThreadState) {
    cx.local_scheduler()
        .spawn(async move || {
            cause_panic();
        })
        .await;
}

#[oxidizer_rt::test]
async fn test_panic_explicit(cx: BasicThreadState) {
    cx.local_scheduler()
        .spawn(async move || {
            if let Err(p) = catch_unwind(cause_panic) {
                let Some(message) = p.downcast_ref::<&str>() else {
                    resume_unwind(p);
                };

                assert_eq!(
                    *message,
                    "this is a panic and we expect it to be visible in the test output"
                );
            }
        })
        .await;
}

fn cause_panic() {
    panic!("this is a panic and we expect it to be visible in the test output")
}