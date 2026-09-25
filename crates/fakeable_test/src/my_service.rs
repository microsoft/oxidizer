// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;

use thread_aware::ThreadAware;

#[fakeable::fakeable(
    fake_impl = fakes::FakeMyService
)]
#[derive(Clone, ThreadAware)]
pub struct MyService {
    value: String,
    other_value: i32,
}

#[fakeable::fakeable]
impl MyService {
    #[must_use]
    pub const fn new(value: String, other_value: i32) -> Self {
        Self { value, other_value }
    }

    pub fn get_value(&self) -> &str {
        self.unsupported("aa");
        Self::unsupported2();
        &self.value
    }

    pub fn special_clone(&self, x: i32) -> Self {
        Self {
            value: self.value.clone(),
            other_value: self.other_value + x,
        }
    }

    pub fn transform(self, x: i32) -> Self {
        Self {
            value: self.value,
            other_value: self.other_value + x,
        }
    }

    #[must_use]
    pub const fn get_other_value(&self) -> i32 {
        self.other_value
    }

    pub fn get_value_with_ref_arg(&self, prefix: &str, suffix: Option<&str>) -> String {
        format!("{}{}{}", prefix, self.value, suffix.unwrap_or(""))
    }

    pub fn process(&self, prefix: impl AsRef<str>) -> String {
        format!("{} Value: {}, Other: {}", prefix.as_ref(), self.value, self.other_value)
    }

    pub const fn mutable(&mut self, increment: i32) {
        self.other_value += increment;
    }

    #[must_use]
    pub fn consume(self) -> String {
        format!("Value: {}, Other: {}", self.value, self.other_value)
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

    const fn unsupported(&self, _input: &str) {
        let _ = self.get_other_value();
    }

    const fn unsupported2() {}
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

        #[must_use]
        pub const fn special_clone(&self, _x: i32) -> Self {
            Self
        }

        #[must_use]
        pub const fn transform(self, _x: i32) -> Self {
            Self
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

        pub const fn mutable(&mut self, _increment: i32) {}

        #[must_use]
        pub fn consume(self) -> String {
            "consumed".into()
        }

        #[allow(clippy::unused_async, clippy::missing_const_for_fn, clippy::no_effect, reason = "Testing function")]
        pub async fn async_function_unit(&self) {}

        #[must_use]
        pub fn get_value_with_ref_arg(&self, prefix: &str, suffix: Option<&str>) -> String {
            format!("{}{}{}", prefix, "fake", suffix.unwrap_or(""))
        }
    }
}
