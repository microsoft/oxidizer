// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    unexpected_cfgs,
    reason = "We want to generate the code for a non-existent feature to test behavior of that branch despite always building with --all-features"
)]

use std::error::Error;

use thread_aware::ThreadAware;

// The unused_fakes feature does not exist in Cargo.toml, so we'll never actually
// utilize the fake branch in test code. This is to test the correct behavior
// when fakes features are disabled.
#[fakeable::fakeable(
    fakes_feature = "unused_fakes",
    fake_impl = fakes::FakeMyService
)]
#[derive(Clone, ThreadAware)]
pub struct MyService {
    value: String,
    other_value: i32,
}

#[fakeable::fakeable(fakes_feature = "unused_fakes")]
impl MyService {
    #[must_use]
    pub const fn new(value: String, other_value: i32) -> Self {
        Self { value, other_value }
    }

    pub fn get_value(&self) -> &str {
        &self.value
    }

    #[must_use]
    pub const fn get_other_value(&self) -> i32 {
        self.other_value
    }

    pub fn process(&self, prefix: impl AsRef<str>) -> String {
        format!("{} Value: {}, Other: {}", prefix.as_ref(), self.value, self.other_value)
    }

    #[allow(
        clippy::unused_async,
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "Tests behavior with async functions"
    )]
    pub async fn async_function(&self, value: i32) -> Result<i32, Box<dyn Error>> {
        Ok(value)
    }

    #[allow(
        clippy::unused_async,
        clippy::unused_self,
        clippy::missing_const_for_fn,
        reason = "Tests behavior with async functions"
    )]
    pub async fn async_function_unit(&self) {}
}

#[cfg(any(feature = "test-util", test))]
pub mod fakes {
    use std::error::Error;

    use thread_aware::ThreadAware;

    #[derive(Clone, ThreadAware)]
    pub struct FakeMyService;

    impl FakeMyService {
        #[must_use]
        pub const fn get_value(&self) -> &'static str {
            "fake"
        }

        #[must_use]
        pub const fn get_other_value(&self) -> i32 {
            42
        }

        pub fn process(&self, prefix: impl AsRef<str>) -> String {
            format!("processed {}", prefix.as_ref())
        }

        #[allow(
            clippy::unused_async,
            clippy::unused_self,
            clippy::unnecessary_wraps,
            reason = "Tests behavior with async functions"
        )]
        pub async fn async_function(&self, value: i32) -> Result<i32, Box<dyn Error>> {
            Ok(42 + value)
        }

        #[allow(clippy::unused_async, clippy::missing_const_for_fn, clippy::no_effect, reason = "Testing function")]
        pub async fn async_function_unit(&self) {}
    }
}
