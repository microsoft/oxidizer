// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use insta::assert_snapshot;
use quote::quote;

use crate as fakeable_impl;

macro_rules! assert_expansion {
    ($tokens:expr) => {
        assert_snapshot!(testing_aids::render_expansion($tokens));
    };
}

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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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

            pub fn marker<T: Default>(&self) -> u32 {
                42
            }

            pub fn lifetime<'a>(&self, value: &'a str) -> &'a str {
                value
            }

            pub fn const_marker<const N: usize>(&self) -> usize {
                N
            }

            pub fn generic_new<T: Default>() -> Self {
                Self {
                    value: String::new(),
                    other_value: 0,
                }
            }

            fn private_method_ignored(&self) { }

            pub(super) fn pub_private_method_accepted(&self) { }
        }
    };

    let args = quote! {};

    let result = fakeable_impl::fakeable_impl(args, input);
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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

            fn output(&self) -> &Something {
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
    assert_expansion!(&result);
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
fn fakeable_rejects_conditional_derive() {
    let input = quote! {
        #[cfg_attr(feature = "clone", derive(Clone))]
        struct MyService {
            value: String,
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { fake_impl = FakeMyService }, input).to_string();

    assert!(result.contains("cfg_attr applying derive is not supported"));
}

#[test]
fn fakeable_rejects_repr_attributes() {
    let input = quote! {
        #[repr(C)]
        struct MyService {
            value: String,
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { fake_impl = FakeMyService }, input).to_string();

    assert!(result.contains("repr attributes are not supported"));
}

#[test]
fn fakeable_rejects_conditional_repr() {
    let input = quote! {
        #[cfg_attr(feature = "ffi", repr(C))]
        struct MyService {
            value: String,
        }

    };

    let result = fakeable_impl::fakeable_impl(quote! { fake_impl = FakeMyService }, input).to_string();

    assert!(result.contains("cfg_attr applying repr is not supported"));
}

#[test]
fn fakeable_accepts_non_repr_cfg_attr() {
    let input = quote! {
        #[cfg_attr(feature = "docs", doc = "service")]
        struct MyService {
            value: String,
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { fake_impl = FakeMyService }, input);

    assert_expansion!(&result);
}

#[test]
fn fakeable_rejects_public_fields() {
    let input = quote! {
        struct MyService {
            pub value: String,
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { fake_impl = FakeMyService }, input).to_string();

    assert!(result.contains("fakeable structs cannot expose fields"));

    let tuple = fakeable_impl::fakeable_impl(
        quote! { fake_impl = FakeMyService },
        quote!(
            struct MyService(pub String);
        ),
    )
    .to_string();
    assert!(tuple.contains("fakeable structs cannot expose fields"));
}

#[test]
fn fakeable_rejects_relative_fake_paths() {
    for path in [quote!(self::fakes::Fake), quote!(super::fakes::Fake)] {
        let result = fakeable_impl::fakeable_impl(
            quote! { fake_impl = #path },
            quote!(
                struct MyService;
            ),
        )
        .to_string();

        assert!(result.contains("paths starting with self or super"));
    }
}

#[test]
fn fakeable_rejects_qualified_impl_targets() {
    let result = fakeable_impl::fakeable_impl(quote! {}, quote!(impl services::MyService {})).to_string();

    assert!(result.contains("qualified impl targets are not supported"));
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
fn fakeable_rejects_projected_self_type() {
    let input = quote! {
        impl MyService {
            pub fn get(&self) -> Self::Assoc {
                todo!()
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();

    assert!(result.contains("nested Self return types are not supported"));
}

#[test]
fn fakeable_rejects_impl_trait_return() {
    for output in [quote! { impl core::fmt::Display }, quote! { (impl core::fmt::Display) }] {
        let input = quote! {
            impl MyService {
                pub fn value(&self) -> #output {
                    42
                }
            }
        };

        let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();
        assert!(result.contains("impl Trait return types are not supported"));
    }
}

#[test]
fn fakeable_rejects_parameter_binding_modifiers() {
    for input in [
        quote! { impl MyService { pub fn set(&self, ref value: String) {} } },
        quote! { impl MyService { pub fn set(&self, ref mut value: String) {} } },
        quote! { impl MyService { pub fn set(&self, value @ Some(_): Option<String>) {} } },
    ] {
        let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();
        assert!(result.contains("ref, ref mut, and subpatterns are not supported"));
    }
}

#[test]
fn fakeable_rejects_parameter_and_generic_attributes() {
    let parameter = fakeable_impl::fakeable_impl(
        quote! {},
        quote! { impl MyService { pub fn set(&self, #[cfg(test)] value: String) {} } },
    )
    .to_string();
    assert!(parameter.contains("attributes on method parameters are not supported"));

    let generic = fakeable_impl::fakeable_impl(
        quote! {},
        quote! { impl MyService { pub fn set<#[cfg(test)] T>(&self, value: T) {} } },
    )
    .to_string();
    assert!(generic.contains("attributes on method generic parameters are not supported"));

    let receiver = fakeable_impl::fakeable_impl(quote! {}, quote! { impl MyService { pub fn value(#[cfg(any())] &self) {} } }).to_string();
    assert!(receiver.contains("attributes on method receivers are not supported"));
}

#[test]
fn fakeable_avoids_const_generic_binding_collisions() {
    let result = fakeable_impl::fakeable_impl(
        quote! {},
        quote! {
            impl MyService {
                pub fn value<const __fakeable_real: usize, const __fakeable_fake: usize>(&self) -> usize {
                    __fakeable_real + __fakeable_fake
                }
            }
        },
    );
    let rendered = testing_aids::render_expansion(&result);
    assert!(rendered.contains("Real(__fakeable_real_1)"));
    assert!(rendered.contains("Fake(__fakeable_fake_1)"));
}

#[test]
fn fakeable_rejects_unsafe_methods() {
    let result = fakeable_impl::fakeable_impl(quote! {}, quote! { impl MyService { pub unsafe fn value(&self) {} } }).to_string();
    assert!(result.contains("unsafe methods are not supported"));
}

#[test]
fn fakeable_rejects_concrete_service_type_at_wrapper_boundary() {
    for input in [
        quote! { impl MyService { pub fn merge(&self, other: Option<MyService>) {} } },
        quote! { impl MyService { pub fn clone_like(&self) -> Option<MyService> { None } } },
        quote! { impl MyService { pub fn value<T: Into<MyService>>(&self, value: T) {} } },
    ] {
        let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();
        assert!(result.contains("concrete service type is not supported"));
    }
}

#[test]
fn fakeable_rejects_public_inherent_associated_items() {
    let associated_const = fakeable_impl::fakeable_impl(quote! {}, quote! { impl MyService { pub const VERSION: u32 = 1; } }).to_string();
    assert!(associated_const.contains("public associated constants"));

    let associated_type = fakeable_impl::fakeable_impl(quote! {}, quote! { impl MyService { pub type Output = u32; } }).to_string();
    assert!(associated_type.contains("public associated types"));
}

#[test]
fn fakeable_keeps_private_inherent_associated_items_hidden() {
    let result = fakeable_impl::fakeable_impl(quote! {}, quote! { impl MyService { const VERSION: u32 = 1; type Output = u32; } });
    let rendered = testing_aids::render_expansion(&result);
    assert_eq!(rendered.matches("VERSION").count(), 1);
    assert_eq!(rendered.matches("type Output").count(), 1);
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
    assert_expansion!(&result);
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
fn fakeable_mockall_rejects_cross_parameter_higher_ranked_shadowing() {
    let input = quote! {
        impl MyService {
            pub fn call(
                &self,
                value: Option<&str>,
                callback: for<'mock> fn(&'mock str),
            ) {
                let _ = value;
                callback("");
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { generate_mockall_fake = true }, input).to_string();

    assert!(result.contains("higher-ranked lifetime binders"));
}

#[test]
fn fakeable_mockall_rejects_consuming_receiver() {
    let input = quote! {
        impl MyService {
            pub fn consume(self) {}
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { generate_mockall_fake = true }, input).to_string();

    assert!(result.contains("does not support consuming self receivers"));
}

#[test]
fn fakeable_mockall_rejects_self_return() {
    let result = fakeable_impl::fakeable_impl(
        quote! { generate_mockall_fake = true },
        quote! { impl MyService { pub fn duplicate(&self) -> Self { todo!() } } },
    )
    .to_string();

    assert!(result.contains("does not support methods returning Self"));
}

#[test]
fn fakeable_mockall_rejects_multiple_nested_elided_references() {
    let result = fakeable_impl::fakeable_impl(
        quote! { generate_mockall_fake = true },
        quote! {
            impl MyService {
                pub fn compare(
                    &self,
                    left: core::cell::Cell<&str>,
                    right: core::cell::Cell<&str>,
                ) {
                }
            }
        },
    )
    .to_string();

    assert!(result.contains("does not support multiple nested elided references"));
}

#[test]
fn fakeable_mockall_accepts_one_nested_elided_reference() {
    let result = fakeable_impl::fakeable_impl(
        quote! { generate_mockall_fake = true },
        quote! {
            impl MyService {
                pub fn inspect(&self, value: core::cell::Cell<&str>) {}
            }
        },
    );

    assert!(!result.to_string().contains("compile_error"));
}

#[test]
fn fakeable_mockall_accepts_bound_lifetime_without_nested_elision() {
    for parameter in [
        quote! { callback: for<'value> fn(&'value str) },
        quote! { callback: Box<dyn for<'value> Fn(&'value str)> },
        quote! { values: Box<dyn Iterator<Item = &str>> },
    ] {
        let result = fakeable_impl::fakeable_impl(
            quote! { generate_mockall_fake = true },
            quote! {
                impl MyService {
                    pub fn call(&self, #parameter) {
                    }
                }
            },
        );

        assert!(!result.to_string().contains("compile_error"));
    }
}

#[test]
fn fakeable_mockall_rejects_const_method() {
    let input = quote! {
        impl MyService {
            pub const fn value(&self) -> i32 {
                42
            }
        }
    };

    let result = fakeable_impl::fakeable_impl(quote! { generate_mockall_fake = true }, input).to_string();

    assert!(result.contains("does not support const methods"));
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
    assert_expansion!(&result);
}

#[test]
fn fakeable_rejects_wrapper_sensitive_trait_arguments() {
    for input in [
        quote! {
            impl Service<MyService> for MyService {
                fn value(&self) {}
            }
        },
        quote! {
            impl Service<Self> for MyService {
                fn value(&self) {}
            }
        },
    ] {
        let result = fakeable_impl::fakeable_impl(quote! {}, input).to_string();
        assert!(result.contains("trait impl paths cannot reference Self or the concrete service type"));
    }
}

#[test]
fn fakeable_mockall_rejects_implicit_higher_ranked_elision() {
    for parameter in [
        quote! { callback: fn(Option<&str>) },
        quote! { callback: Box<dyn Fn(Option<&str>)> },
    ] {
        let result = fakeable_impl::fakeable_impl(
            quote! { generate_mockall_fake = true },
            quote! {
                impl MyService {
                    pub fn call(&self, #parameter) {}
                }
            },
        )
        .to_string();

        assert!(result.contains("implicit higher-ranked function or trait-object binders"));
    }
}

#[test]
fn fakeable_mockall_preserves_method_cfg_attributes() {
    let result = fakeable_impl::fakeable_impl(
        quote! { generate_mockall_fake = true },
        quote! {
            impl MyService {
                #[cfg(feature = "tls")]
                #[cfg_attr(feature = "docs", doc = "TLS")]
                pub fn connect(&self, config: TlsConfig) {}
            }
        },
    );
    let tokens = result.to_string();

    assert_eq!(tokens.matches("\"tls\"").count(), 3);
    assert_eq!(tokens.matches("\"docs\"").count(), 3);
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
