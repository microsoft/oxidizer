// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use insta::assert_snapshot;
use quote::quote;

use crate as fakeable_impl;

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
fn fakeable_on_impl_with_async_constructor_generates_expected_code() {
    let input = quote! {
        impl MyService {
            pub async fn new(value: String) -> Self {
                Self { value }
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input);
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
        impl Service for MyService {
            type Output = Something;

            const NAME: &'static str = "my-service";

            fn create(something: Something) -> Self {
                Self { something }
            }

            fn output(&self) -> &Self::Output {
                &self.something
            }

            fn duplicate(&self) -> Self {
                Self {
                    something: self.something.clone(),
                }
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

            pub async fn async_clone(&self) -> Self {
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

#[test]
fn fakeable_on_generic_impl_preserves_type_arguments() {
    let input = quote! {
        impl<T> MyService<T>
        where
            T: Clone,
        {
            pub fn new(value: T) -> Self {
                Self { value }
            }

            pub fn value(&self) -> &T {
                &self.value
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_on_bounded_generic_struct_preserves_generic_forms() {
    let input = quote! {
        struct MyService<T: Clone>
        where
            T: Send,
        {
            value: T,
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { fake_impl = FakeMyService<T> }, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_on_impl_with_mockall_preserves_async_where_clause() {
    let input = quote! {
        impl MyService {
            pub async fn process<T>(&self, value: T) -> T
            where
                T: Send + 'static,
            {
                value
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
fn fakeable_on_impl_with_mockall_rejects_mutable_receiver() {
    let input = quote! {
        impl MyService {
            pub fn update(&mut self) {}
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { generate_mockall_fake = true }, input).to_string();

    assert!(result.contains("does not support mutable receiver methods"));
}

#[test]
fn fakeable_on_impl_with_mockall_rejects_restricted_visibility() {
    let input = quote! {
        impl MyService {
            pub(crate) fn value(&self) -> i32 {
                42
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { generate_mockall_fake = true }, input).to_string();

    assert!(result.contains("does not support restricted method visibility"));
}

#[test]
fn fakeable_on_impl_rejects_typed_self_receiver() {
    let input = quote! {
        impl MyService {
            pub fn take(self: Box<Self>) {}
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();

    assert!(result.contains("typed self receivers are not supported"));
}

#[test]
fn fakeable_on_generic_impl_with_mockall_is_rejected() {
    let input = quote! {
        impl<T> MyService<T> {
            pub fn value(&self) -> &T {
                &self.value
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { generate_mockall_fake = true }, input).to_string();

    assert!(result.contains("does not support generic impl blocks"));
}

#[test]
fn fakeable_on_trait_impl_with_mockall_is_rejected() {
    let input = quote! {
        impl Service for MyService {
            fn value(&self) -> i32 {
                42
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { generate_mockall_fake = true }, input).to_string();

    assert!(result.contains("does not support trait impl blocks"));
}

#[test]
fn fakeable_on_cfg_struct_gates_generated_items() {
    let input = quote! {
        #[cfg(feature = "enabled")]
        struct MyService {
            value: String,
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { fake_impl = FakeMyService }, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_rejects_cfg_attr_that_can_disable_struct() {
    let input = quote! {
        #[cfg_attr(feature = "conditional", cfg(feature = "enabled"))]
        struct MyService {
            value: String,
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { fake_impl = FakeMyService }, input).to_string();

    assert!(result.contains("cfg_attr applying cfg is not supported"));
}

#[test]
fn fakeable_rejects_unsafe_impl() {
    let input = quote! {
        unsafe impl Send for MyService {}
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();

    assert!(result.contains("unsafe impl blocks are not supported"));
}

#[test]
fn fakeable_rejects_mut_self_receiver() {
    let input = quote! {
        impl MyService {
            pub fn consume(mut self) {
                self.value.clear();
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();

    assert!(result.contains("mut self receivers are not supported"));
}

#[test]
fn fakeable_rejects_self_parameter() {
    let input = quote! {
        impl MyService {
            pub fn merge(self, other: Self) -> Self {
                other
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();

    assert!(result.contains("Self in method parameters is not supported"));
}

#[test]
fn fakeable_rejects_self_in_method_generic_bounds() {
    let input = quote! {
        impl MyService {
            pub fn consume<T: Marker<Self>>(&self, value: T) {
                let _ = value;
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();

    assert!(result.contains("Self in method generic bounds or where predicates is not supported"));
}

#[test]
fn fakeable_rejects_self_in_method_where_predicate() {
    let input = quote! {
        impl MyService {
            pub fn consume<T>(&self, value: T)
            where
                T: Marker<Self>,
            {
                let _ = value;
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();

    assert!(result.contains("Self in method generic bounds or where predicates is not supported"));
}

#[test]
fn fakeable_rejects_nested_self_return() {
    let input = quote! {
        impl MyService {
            pub fn maybe(&self) -> Option<Self> {
                None
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();

    assert!(result.contains("nested Self return types are not supported"));
}

#[test]
fn fakeable_on_cfg_impl_gates_generated_mockall_module() {
    let input = quote! {
        #[cfg(feature = "enabled")]
        impl MyService {
            pub fn value(&self) -> i32 {
                42
            }

        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { generate_mockall_fake = true }, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_mockall_rejects_higher_ranked_nested_elision() {
    let input = quote! {
        impl MyService {
            pub fn call(&self, callback: for<'mock> fn(Option<&str>)) {
                callback(None);
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { generate_mockall_fake = true }, input).to_string();

    assert!(result.contains("higher-ranked lifetime binders"));
}

#[test]
fn fakeable_on_generic_trait_impl_preserves_hidden_type_arguments() {
    let input = quote! {
        impl<T> Service for MyService<T>
        where
            T: Clone,
        {
            fn value(&self) -> i32 {
                42
            }

            fn duplicate(&self) -> Self {
                Self {
                    value: self.value.clone(),
                }
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input);
    let result_file = syn::parse_file(&result.to_string()).unwrap();
    assert_snapshot!(prettyplease::unparse(&result_file));
}

#[test]
fn fakeable_rejects_cfg_attr_that_can_disable_impl() {
    let input = quote! {
        #[cfg_attr(feature = "conditional", cfg(feature = "enabled"))]
        impl MyService {
            pub fn value(&self) -> i32 {
                42
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();

    assert!(result.contains("cfg_attr applying cfg is not supported"));
}
