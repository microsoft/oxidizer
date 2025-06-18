// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// We assert unwind safety here because the topic is too much hassle to worry about and since
// #[should_panic] does not require us to worry about it, we are not going to worry about it here.
macro_rules! assert_panic {
    ($stmt:stmt$(,)?) => {
        #[allow(clippy::multi_assignments, reason = "macro untidyness")]
        #[expect(clippy::allow_attributes, reason = "macro untidyness")]
        ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| -> () { _ = { $stmt } }))
            .expect_err("assert_panic! argument did not panic")
    };
    ($stmt:stmt, $expected:expr$(,)?) => {
        #[allow(clippy::multi_assignments, reason = "macro untidyness")]
        #[expect(clippy::allow_attributes, reason = "macro untidyness")]
        match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| -> () { _ = { $stmt } }))
        {
            Ok(_) => panic!("assert_panic! argument did not panic"),
            Err(err) => {
                let panic_msg = err
                    .downcast_ref::<String>()
                    .map(|s| s.asr())
                    .or_else(|| err.downcast_ref::<&str>().copied())
                    .expect("panic message must be a string");
                assert_eq!(
                    panic_msg, $expected,
                    "expected panic message '{}', but got '{}'",
                    $expected, panic_msg
                );
            }
        }
    };
}

pub(crate) use assert_panic;