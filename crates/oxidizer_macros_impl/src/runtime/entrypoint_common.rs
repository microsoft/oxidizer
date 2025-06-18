// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;

use darling::FromMeta;
use darling::ast::NestedMeta;
use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::{Expr, ExprTry, ItemFn, Path, parse_quote, parse2};

use crate::syn_helpers::bail;

#[derive(Debug, FromMeta)]
struct Args {
    /// The function to call to build the data.
    /// Function signature is `fn foo(rdb: &mut RuntimeDataBuilder) -> StructWithKeys`
    /// Default `None` means no scoped storage is initialized.
    data_fn: Option<Expr>,
    /// Path to runtime/app crate (for situations where the runtime/app is reexported or aliased)
    /// Default `None` means `::oxidizer_rt`
    /// Example: `::reexporting_crate::oxidizer_rt`
    runtime_path: Option<Path>,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "Convention for syn-based code"
)]
#[expect(clippy::too_many_lines, reason = "TODO: add justification")]
pub fn common_codegen(
    args: TokenStream,
    item: TokenStream,
    test: bool,
    // This is not allowed when the macro is exported from the runtime crate as runtime_data are not available.
    allow_data_fn: bool,
    default_runtime_path: Path,
) -> TokenStream {
    let mut input: ItemFn = match parse2(item.clone()) {
        Ok(v) => v,
        Err(e) => {
            return e.to_compile_error();
        }
    };

    let output = &input.sig.output;
    let returns_error = match output {
        syn::ReturnType::Type(_, ty) => ty.to_token_stream().to_string().contains("Result"),
        syn::ReturnType::Default => false,
    };
    let mut attrs = input.attrs;
    let vis = &input.vis;
    let sig = &mut input.sig;
    let body = &input.block;

    let mut input_args = VecDeque::from_iter(sig.inputs.clone());

    let arg_list = match NestedMeta::parse_meta_list(args.clone()) {
        Ok(v) => v,
        Err(e) => {
            return darling::Error::from(e).write_errors();
        }
    };

    let macro_args = match Args::from_list(&arg_list) {
        Ok(v) => v,
        Err(e) => {
            return e.write_errors();
        }
    };

    let runtime_path = macro_args.runtime_path.unwrap_or(default_runtime_path);

    if sig.asyncness.is_none() {
        bail!(
            item,
            sig.fn_token,
            "function must be async to use the attribute"
        );
    }

    let Some(syn::FnArg::Typed(thread_state)) = input_args.pop_front() else {
        bail!(
            item,
            sig.fn_token,
            "function must take a TaskContext argument"
        );
    };

    let syn::Pat::Ident(thread_state_ident) = thread_state.pat.as_ref() else {
        bail!(item, &thread_state.pat, "argument must have an identifier");
    };

    let syn::Type::Path(syn::TypePath {
        path: thread_state_type,
        ..
    }) = thread_state.ty.as_ref()
    else {
        bail!(item, &thread_state.ty, "argument type must be Type::Path");
    };

    let shared_state_init = match macro_args.data_fn {
        None => {
            quote! {
                let shared_state = ::std::default::Default::default();
            }
        }
        Some(data_fn) => {
            if thread_state_type
                .segments
                .last()
                .is_none_or(|segment| segment.ident != "TaskContext")
            {
                bail!(
                    item,
                    &thread_state_type,
                    "when data_fn is used, argument type must be TaskContext"
                );
            }

            if !allow_data_fn {
                bail!(
                    item,
                    &args,
                    "data_fn is not supported with macros based on rt crate/module (use macros from app crate/module)"
                );
            }

            let Some(syn::FnArg::Typed(keys_arg)) = input_args.pop_front() else {
                bail!(
                    item,
                    sig.fn_token,
                    "function must take a struct containing scoped store keys as an argument"
                )
            };
            let keys_ident = &keys_arg.pat;
            let keys_type = &keys_arg.ty;

            let mut questionmark = TokenStream::new();

            // Check if the data_fn is suffixed by ? to indicate that it returns a Result
            let data_fn = match data_fn {
                Expr::Path(path) => path.path,
                Expr::Try(ExprTry { expr, .. }) => {
                    if let Expr::Path(path) = expr.as_ref() {
                        if !returns_error {
                            bail!(
                                item,
                                &args,
                                "Fallible data function requires main that returns Result"
                            );
                        }
                        questionmark = quote! { ? };
                        path.path.clone()
                    } else {
                        bail!(item, &args, "data_fn must be a function path")
                    }
                }
                _ => bail!(item, &args, "data_fn must be a function path"),
            };
            quote! {
                let mut rdb = #runtime_path::task_context::scoped::RuntimeDataBuilder::new();
                let #keys_ident: #keys_type = #data_fn(&mut rdb)#questionmark;
                let shared_state = #runtime_path::task_context::TaskContextRuntimeBuilder::new()
                    .with_data(rdb).into();
            }
        }
    };

    if test {
        attrs.push(parse_quote!(#[::core::prelude::v1::test]));
    }

    if !input_args.is_empty() {
        bail!(item, input_args.front().unwrap(), "unexpected arguments");
    }

    sig.asyncness = None;
    sig.inputs.clear();

    quote! {
        #(#attrs)*
        #vis #sig {
            #shared_state_init
            #runtime_path::Runtime::with_shared_state(shared_state)
                .expect("Failed to create runtime")
                .run(async move |#thread_state_ident: #thread_state_type| #body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syn_helpers::contains_compile_error;

    #[expect(
        clippy::needless_pass_by_value,
        reason = "Convention for syn-based code"
    )]
    fn assert_codegen_pass(input: TokenStream, args: TokenStream) {
        for test in [true, false] {
            let output = common_codegen(
                args.clone(),
                input.clone(),
                test,
                true,
                parse_quote!(::oxidizer_app),
            );
            assert!(
                !contains_compile_error(&output),
                "Failed for test = {test:?} \n \n {output}"
            );
        }
    }

    #[test]
    fn smoke_test() {
        let input = quote! {
            async fn my_test(_cx: TaskContext) {
                println!("Hello, world!");
                TaskContext::yield_now().await;
            }
        };
        let args = TokenStream::new();
        assert_codegen_pass(input, args);
    }

    #[test]
    fn smoke_test_data() {
        let input = quote! {
            async fn my_test(_cx: TaskContext, keys: FakeKeyStruct) {
                println!("Hello, world!");
                TaskContext::yield_now().await;
            }
        };
        let args = parse_quote!(data_fn = foobar);
        assert_codegen_pass(input, args);
    }
}