// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// We assert unwind safety here because the topic is too much hassle to worry about and since
// #[should_panic] does not require us to worry about it, we are not going to worry about it here.
#[macro_export]
macro_rules! assert_panic {
    ($stmt:stmt$(,)?) => {
        #[allow(clippy::multi_assignments, reason = "macro untidiness")]
        #[expect(clippy::allow_attributes, reason = "macro untidiness")]
        ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| -> () { _ = { $stmt } }))
            .expect_err("assert_panic! argument did not panic")
    };
}
