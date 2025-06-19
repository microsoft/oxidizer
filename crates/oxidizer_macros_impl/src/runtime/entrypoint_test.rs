// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use syn::parse_quote;

use super::entrypoint_common::common_codegen;

#[must_use]
pub fn impl_runtime_test(args: TokenStream, item: TokenStream) -> TokenStream {
    common_codegen(args, item, true, false, parse_quote!(::oxidizer_rt))
}

#[must_use]
pub fn impl_app_test(args: TokenStream, item: TokenStream) -> TokenStream {
    common_codegen(args, item, true, true, parse_quote!(::oxidizer_app))
}

#[must_use]
pub fn impl_oxidizer_app_test(args: TokenStream, item: TokenStream) -> TokenStream {
    common_codegen(args, item, true, true, parse_quote!(::oxidizer::app))
}

#[cfg(not(miri))] // Insta does not work under Miri.
#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn test() {
        let input = quote! {
            async fn test_case(_) {
                println!("Hello, world!");
                TaskContext::yield_now().await;
            }
        };
        let args = TokenStream::new();
        let result = impl_runtime_test(args.clone(), input.clone());
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        ::core::compile_error! {
            "expected `:`"
        }
        "#);

        let result = impl_app_test(args.clone(), input.clone());
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        ::core::compile_error! {
            "expected `:`"
        }
        "#);

        let result = impl_oxidizer_app_test(args, input);
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        ::core::compile_error! {
            "expected `:`"
        }
        "#);
    }

    #[test]
    #[expect(clippy::literal_string_with_formatting_args, reason = "By design")]
    fn test_data() {
        let input = quote! {
            async fn test_case(cx: TaskContext, keys: Keys) {
                let instance_a = cx.instance_of(keys.key_a);
                println!("test_instance: {instance_a:?}!");
            }
        };
        let args = parse_quote!(data_fn = build_data_fn);
        let result = impl_app_test(args, input);
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        #[::core::prelude::v1::test]
        fn test_case() {
            let mut rdb = ::oxidizer_app::task_context::scoped::RuntimeDataBuilder::new();
            let keys: Keys = build_data_fn(&mut rdb);
            let shared_state = ::oxidizer_app::task_context::TaskContextRuntimeBuilder::new()
                .with_data(rdb)
                .into();
            ::oxidizer_app::Runtime::with_shared_state(shared_state)
                .expect("Failed to create runtime")
                .run(async move |cx: TaskContext| {
                    let instance_a = cx.instance_of(keys.key_a);
                    println!("test_instance: {instance_a:?}!");
                })
        }
        "#);
    }

    #[test]
    #[expect(clippy::literal_string_with_formatting_args, reason = "By design")]
    fn test_rt_data_compile_error() {
        let input = quote! {
            async fn test_case(cx: TaskContext, keys: Keys) {
                let instance_a = cx.instance_of(keys.key_a);
                println!("test_instance: {instance_a:?}!");
            }
        };
        let args = parse_quote!(data_fn = build_data_fn);
        let result = impl_runtime_test(args, input);
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        async fn test_case(cx: TaskContext, keys: Keys) {
            let instance_a = cx.instance_of(keys.key_a);
            println!("test_instance: {instance_a:?}!");
        }
        ::core::compile_error! {
            "data_fn is not supported with macros based on rt crate/module (use macros from app crate/module)"
        }
        "#);
    }
}