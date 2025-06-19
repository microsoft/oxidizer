// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::TokenStream;
use syn::parse_quote;

use super::entrypoint_common::common_codegen;

#[must_use]
pub fn impl_runtime_main(args: TokenStream, item: TokenStream) -> TokenStream {
    common_codegen(args, item, false, false, parse_quote!(::oxidizer_rt))
}
#[must_use]
pub fn impl_app_main(args: TokenStream, item: TokenStream) -> TokenStream {
    common_codegen(args, item, false, true, parse_quote!(::oxidizer_app))
}
#[must_use]
pub fn impl_oxidizer_app_main(args: TokenStream, item: TokenStream) -> TokenStream {
    common_codegen(args, item, false, true, parse_quote!(::oxidizer::app))
}

#[cfg(not(miri))] // Insta does not work under Miri.
#[cfg(test)]
#[expect(clippy::literal_string_with_formatting_args, reason = "By design")]
mod tests {
    use insta::assert_snapshot;
    use quote::quote;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn test_main() {
        let input = quote! {
            async fn main(cx: TaskContext) {
                println!("Hello, world!");
                cx::yield_now().await;
            }
        };
        let args = TokenStream::new();
        let result = impl_runtime_main(args.clone(), input.clone());
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        fn main() {
            let shared_state = ::std::default::Default::default();
            ::oxidizer_rt::Runtime::with_shared_state(shared_state)
                .expect("Failed to create runtime")
                .run(async move |cx: TaskContext| {
                    println!("Hello, world!");
                    cx::yield_now().await;
                })
        }
        "#);

        let result = impl_app_main(args.clone(), input.clone());
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        fn main() {
            let shared_state = ::std::default::Default::default();
            ::oxidizer_app::Runtime::with_shared_state(shared_state)
                .expect("Failed to create runtime")
                .run(async move |cx: TaskContext| {
                    println!("Hello, world!");
                    cx::yield_now().await;
                })
        }
        "#);

        let result = impl_oxidizer_app_main(args, input);
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        fn main() {
            let shared_state = ::std::default::Default::default();
            ::oxidizer::app::Runtime::with_shared_state(shared_state)
                .expect("Failed to create runtime")
                .run(async move |cx: TaskContext| {
                    println!("Hello, world!");
                    cx::yield_now().await;
                })
        }
        "#);
    }

    #[test]
    fn test_main_data() {
        let input = quote! {
            async fn main(cx: TaskContext, keys: Keys) {
                let instance_a = cx.instance_of(keys.key_a);
                println!("test_instance: {instance_a:?}!");
            }
        };
        let args = parse_quote!(data_fn = build_data_fn);
        let result = impl_app_main(args, input);
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        fn main() {
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
    fn test_main_rt_data_compile_error() {
        let input = quote! {
            async fn main(cx: TaskContext, keys: Keys) {
                let instance_a = cx.instance_of(keys.key_a);
                println!("test_instance: {instance_a:?}!");
            }
        };
        let args = parse_quote!(data_fn = build_data_fn);
        let result = impl_app_main(args, input);
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        fn main() {
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
    fn test_main_data_try() {
        let input = quote! {
            async fn main(cx: TaskContext, keys: Keys) -> anyhow::Result<()>{
                let instance_a = cx.instance_of(keys.key_a);
                println!("test_instance: {instance_a:?}!");
            }
        };
        let args = parse_quote!(data_fn = build_data_fn?);
        let result = impl_app_main(args, input);
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        fn main() -> anyhow::Result<()> {
            let mut rdb = ::oxidizer_app::task_context::scoped::RuntimeDataBuilder::new();
            let keys: Keys = build_data_fn(&mut rdb)?;
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
    fn test_main_runtime_elsewhere() {
        let input = quote! {
            async fn main(_) {
                println!("Hello, world!");
                TaskContext::yield_now().await;
            }
        };
        let args = parse_quote!(runtime_path = ::path::to::reexported::runtime);
        let result = impl_app_main(args, input);
        let result_file = syn::parse_file(&result.to_string()).unwrap();
        assert_snapshot!(prettyplease::unparse(&result_file), @r#"
        ::core::compile_error! {
            "expected `:`"
        }
        "#);
    }
}