// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use insta::assert_snapshot;
use quote::quote;

#[test]
fn fakeable_on_struct_generates_expected_code() {
    let input = quote! {
        struct MyService {
            value: String,
            other_value: i32,
        }
    };

    let args = quote! {
        fake_impl = fakes::FakeMyService
    };

    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_on_struct_generates_expected_code_with_accessibility() {
    let input = quote! {
        pub(super) struct MyService {
            value: String,
            other_value: i32,
        }
    };

    let args = quote! {
        fake_impl = fakes::FakeMyService
    };

    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_on_struct_generates_expected_code_with_derive() {
    let input = quote! {
        #[derive(Clone, ThreadAware)]
        pub(super) struct MyService {
            value: String,
            other_value: i32,
        }
    };

    let args = quote! {
        fakes_attribute = "fakes",
        fake_impl = fakes::FakeMyService,
        fake_constructor = "fake"
    };

    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_on_impl_generates_expected_code() {
    let input = quote! {
        impl MyService {
            fn new(value: String, other_value: i32) -> Self {
                Self { value, other_value }
            }

            pub fn new2(value: String) -> Self {
                Self { value, other_value: 0 }
            }

            pub fn get_value(&self) -> &str {
                &self.value
            }

            pub fn get_other_value(&self) -> i32 {
                self.other_value
            }

            #[must_use]
            pub fn self_fn(self) -> Self {
                Self { value: self.value, other_value: self.other_value }
            }

            pub fn self_fn_consuming(self) -> String {
                format!("Value: {}, Other: {}", self.value, self.other_value)
            }

            pub fn process(&self, prefix: impl AsRef<str>) -> String {
                format!(
                    "{} Value: {}, Other: {}",
                    prefix.as_ref(),
                    self.value,
                    self.other_value
                )
            }

            fn private_method_ignored(&self) { }

            pub(super) fn pub_private_method_accepted(&self) { }
        }
    };

    let args = quote! {};

    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_on_impl_with_mockall_generates_expected_code() {
    let input = quote! {
        impl MyService {
            fn new(value: String, other_value: i32) -> Self {
                Self { value, other_value }
            }

            pub fn new2(value: String) -> Self {
                Self { value, other_value: 0 }
            }

            pub fn get_value(&self) -> &str {
                &self.value
            }

            pub fn get_other_value(&self) -> i32 {
                self.other_value
            }

            pub fn get_value_with_ref_arg(&self, prefix: &str, suffix: Option<&str>) -> String {
                format!("{}{}{}", prefix, self.value, suffix.unwrap_or(""))
            }
        }
    };

    let args = quote! {
        generate_mockall_fake = true
    };

    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_on_impl_with_mockall_current_module_generates_expected_code() {
    let input = quote! {
        impl MyService {
            fn new(value: String, other_value: i32) -> Self {
                Self { value, other_value }
            }

            pub fn new2(value: String) -> Self {
                Self { value, other_value: 0 }
            }

            pub fn get_value(&self) -> &str {
                &self.value
            }

            pub fn get_other_value(&self) -> i32 {
                self.other_value
            }
        }
    };

    let args = quote! {
        generate_mockall_fake = true,
        mockall_fake_module = "."
    };

    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_on_impl_with_async_generates_expected_code() {
    let input = quote! {
        impl MyService {
            pub fn sync_method(&self) -> String {
                "sync result".to_string()
            }

            pub async fn async_method(&self, value: i32) -> Result<String, Box<dyn std::error::Error>> {
                Ok(format!("async result: {}", value))
            }

            pub async fn async_method_unit(&self) {
                println!("async method unit");
            }
        }
    };

    let args = quote! {};

    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_on_impl_with_mockall_and_async_unit_function_generates_expected_code() {
    let input = quote! {
        impl MyService {
            pub async fn async_method_unit(&self) {
                println!("async method unit");
            }

            pub async fn async_method_return_value(&self) -> Result<(), SomeError> {
                Ok(())
            }
        }
    };

    let args = quote! {
        generate_mockall_fake = true
    };

    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_on_trait_impl_generated_expected_code() {
    let input = quote! {
        impl From<Something> for MyService {
            fn from(something: Something) -> Self {
                Self { something }
            }
        }
    };

    let args = quote! {};

    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_struct_with_expect() {
    let input = quote! {
        #[expect(dead_code)]
        #[allow(whatever)]
        struct MyService {
            #[expect(unused)]
            #[expect(another_expectation, reason = "demonstration")]
            #[cfg_attr(test, expect(dead_code))]
            #[cfg_attr(feature = "some_feature", expect(some_expectation, another_expectation))]
            #[allow(whatever2)]
            value: String,

            other_value: i32,
        }
    };

    let args = quote! {
        fake_impl = fakes::FakeMyService
    };

    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_impl_with_expect() {
    let input = quote! {
        #[expect(dead_code)]
        #[allow(whatever)]
        impl MyService {
            #[expect(unused_async)]
            #[expect(unused_self, reason = "demonstration")]
            #[expect(excpectation1, expectaion2)]
            #[cfg_attr(test, expect(dead_code))]
            #[cfg_attr(feature = "some_feature", expect(some_expectation, another_expectation))]
            #[allow(whatever)]
            #[cfg_attr(test, allow(whatever2))]
            pub async fn get_value(&self) -> &str {
                #[cfg_attr(test)]
                {
                    let a = 5;
                }
                "value"
            }
        }
    };

    let args = quote! {};
    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_on_impl_with_ref_receiver_returning_self_generates_expected_code() {
    let input = quote! {
        impl MyService {
            pub fn new(value: String) -> Self {
                Self { value }
            }

            pub fn special_clone(&self) -> Self {
                Self { value: self.value.clone() }
            }

            pub fn mut_transform(&mut self) -> Self {
                Self { value: self.value.clone() }
            }

            pub fn get_value(&self) -> &str {
                &self.value
            }
        }
    };

    let args = quote! {};

    let result = fakeable_impl::fakeable_impl(args, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}
