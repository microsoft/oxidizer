// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![forbid(unsafe_code)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Implementation of [`arty_macros`](https://docs.rs/arty_macros).
//!
//! Applications should use the entry-point macros re-exported by `arty`.

use darling::FromMeta;
use darling::ast::NestedMeta;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemFn, Path, parse_quote, parse2};

#[derive(Debug, FromMeta)]
struct Args {
    runtime_path: Option<Path>,
}

/// Expands the asynchronous runtime entry-point attribute.
#[must_use]
pub fn main(args: TokenStream, item: TokenStream) -> TokenStream {
    entrypoint(args, item, false)
}

/// Expands the asynchronous runtime test attribute.
#[must_use]
pub fn test(args: TokenStream, item: TokenStream) -> TokenStream {
    entrypoint(args, item, true)
}

fn entrypoint(args: TokenStream, item: TokenStream, test: bool) -> TokenStream {
    let mut input: ItemFn = match parse2(item.clone()) {
        Ok(input) => input,
        Err(error) => return error.to_compile_error(),
    };
    let args = match NestedMeta::parse_meta_list(args) {
        Ok(args) => args,
        Err(error) => return darling::Error::from(error).write_errors(),
    };
    let args = match Args::from_list(&args) {
        Ok(args) => args,
        Err(error) => return error.write_errors(),
    };
    let runtime_path = args.runtime_path.unwrap_or_else(|| parse_quote!(::arty::runtime));
    let sig = &mut input.sig;
    let mut inputs = sig.inputs.iter();
    let fail = move |error: syn::Error| {
        let error = error.to_compile_error();
        quote! { #item #error }
    };
    if sig.asyncness.is_none() {
        return fail(syn::Error::new_spanned(sig.fn_token, "function must be async to use the attribute"));
    }
    let Some(syn::FnArg::Typed(state)) = inputs.next() else {
        return fail(syn::Error::new_spanned(sig.fn_token, "function must take a Builtins argument"));
    };
    let syn::Pat::Ident(state_ident) = state.pat.as_ref() else {
        return fail(syn::Error::new_spanned(&state.pat, "argument must have an identifier"));
    };
    let syn::Type::Path(state_type) = state.ty.as_ref() else {
        return fail(syn::Error::new_spanned(&state.ty, "argument type must be Type::Path"));
    };
    if let Some(extra) = inputs.next() {
        return fail(syn::Error::new_spanned(extra, "unexpected arguments"));
    }
    let state_ident = state_ident.clone();
    let state_type = state_type.clone();
    sig.asyncness = None;
    sig.inputs.clear();
    let mut attrs = input.attrs;
    if test {
        attrs.push(parse_quote!(#[::core::prelude::v1::test]));
    }
    let visibility = input.vis;
    let body = input.block;
    quote! {
        #(#attrs)*
        #visibility #sig {
            #runtime_path::Runtime::new()
                .expect("failed to create the runtime for the entry point")
                .run(async move |#state_ident: #state_type| #body)
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use insta::assert_snapshot;
    use testing_aids::render_expansion;

    use super::*;

    #[test]
    fn main_preserves_visibility_and_return_type() {
        let expansion = main(
            TokenStream::new(),
            quote! {
                pub async fn main(cx: arty::runtime::Builtins) -> Result<(), Error> {
                    run(cx).await
                }
            },
        );
        assert_snapshot!(render_expansion(&expansion), @r#"
        pub fn main() -> Result<(), Error> {
            ::arty::runtime::Runtime::new()
                .expect("failed to create the runtime for the entry point")
                .run(async move |cx: arty::runtime::Builtins| { run(cx).await })
        }
        "#);
    }

    #[test]
    fn test_preserves_attributes_and_runtime_override() {
        let expansion = test(
            quote!(runtime_path = ::renamed),
            quote! {
                #[should_panic(expected = "original payload")]
                async fn fails(mut cx: renamed::Builtins) {
                    fail(&mut cx).await;
                }
            },
        );
        assert_snapshot!(render_expansion(&expansion), @r#"
        #[should_panic(expected = "original payload")]
        #[::core::prelude::v1::test]
        fn fails() {
            ::renamed::Runtime::new()
                .expect("failed to create the runtime for the entry point")
                .run(async move |mut cx: renamed::Builtins| {
                    fail(&mut cx).await;
                })
        }
        "#);
    }

    #[test]
    fn invalid_signatures_preserve_input_and_report_errors() {
        let cases = [
            (
                quote!(
                    fn run(cx: Builtins) {}
                ),
                "function must be async",
            ),
            (
                quote!(
                    async fn run() {}
                ),
                "function must take a Builtins argument",
            ),
            (
                quote!(
                    async fn run(_: Builtins) {}
                ),
                "argument must have an identifier",
            ),
            (
                quote!(
                    async fn run(cx: &Builtins) {}
                ),
                "argument type must be Type::Path",
            ),
            (
                quote!(
                    async fn run(cx: Builtins, extra: ()) {}
                ),
                "unexpected arguments",
            ),
        ];
        for (input, message) in cases {
            for expand in [main, test] {
                let expansion = expand(TokenStream::new(), input.clone()).to_string();
                assert!(expansion.starts_with(&input.to_string()));
                assert!(expansion.contains(message), "{expansion}");
                assert!(expansion.contains("compile_error"));
            }
        }
    }

    #[test]
    fn malformed_items_and_arguments_report_errors() {
        for (args, input) in [
            (TokenStream::new(), quote!(not a function)),
            (
                quote!(@),
                quote!(
                    async fn run(cx: Builtins) {}
                ),
            ),
            (
                quote!(runtime_path =),
                quote!(
                    async fn run(cx: Builtins) {}
                ),
            ),
            (
                quote!(unknown = 1),
                quote!(
                    async fn run(cx: Builtins) {}
                ),
            ),
            (
                quote!(runtime_path = ::a, runtime_path = ::b),
                quote!(
                    async fn run(cx: Builtins) {}
                ),
            ),
        ] {
            assert!(main(args, input).to_string().contains("compile_error"));
        }
    }
}
