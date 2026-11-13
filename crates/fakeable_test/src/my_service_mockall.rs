// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

#[allow(dead_code, reason = "Verifies generated `::mockall` paths cannot be shadowed")]
mod mockall {}

#[fakeable::fakeable(
    fake_impl = thread_aware::Unaware<std::sync::Arc<fakes::MockMyService>>
)]
#[derive(Clone, ThreadAware)]
pub struct MyService {
    value: String,
    other_value: i32,
}

#[fakeable::fakeable(generate_mockall_fake = true)]
impl MyService {
    #[cfg(any())]
    pub fn disabled_consume(self) {}

    pub const fn new(value: String, other_value: i32) -> Self {
        Self { value, other_value }
    }

    pub fn get_value(&self) -> &str {
        &self.value
    }

    pub fn get_value_with_ref_arg(&self, prefix: &str, suffix: Option<&str>) -> String {
        format!("{}{}{}", prefix, self.value, suffix.unwrap_or(""))
    }

    #[allow(clippy::missing_const_for_fn, reason = "Used by mockall")]
    pub fn get_other_value(&self) -> i32 {
        // #[gamma::skip(stmt.delete_call, literal.str_to_empty, literal.str_to_xyzzy, tag = "equivalent", reason = "This real fixture helper ignores its argument and only borrows its immutable value field without side effects.")]
        self.unsupported("aa");
        // #[gamma::skip(stmt.delete_call, tag = "equivalent", reason = "The fixture helper has an empty const body and returns unit.")]
        Self::unsupported2();
        self.other_value
    }

    #[allow(
        clippy::unused_async,
        clippy::unused_self,
        clippy::missing_const_for_fn,
        clippy::unnecessary_wraps,
        reason = "Tests behavior with async functions"
    )]
    pub async fn async_function(&self, value: i32) -> Result<i32, std::io::Error> {
        Ok(value)
    }

    #[allow(
        clippy::unused_async,
        clippy::unused_self,
        clippy::missing_const_for_fn,
        reason = "Tests behavior with async functions"
    )]
    pub async fn async_function_unit(&self) {}

    // #[gamma::skip(fn_value.unit, tag = "equivalent", reason = "This helper only borrows the real fixture's value field and discards the reference; it has no side effects.")]
    fn unsupported(&self, _input: &str) {
        let _ = self.get_value();
    }

    const fn unsupported2() {}
}
