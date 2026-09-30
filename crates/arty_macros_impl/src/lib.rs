// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![forbid(unsafe_code)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Implementation of [`arty_macros`](https://docs.rs/arty_macros).
//!
//! Applications should use the entry-point macros re-exported by `arty`.

use darling::FromMeta;
use darling::ast::NestedMeta;
use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::ext::IdentExt;
use syn::{Expr, Ident, ItemFn, Lit, Path, parse_quote, parse2};

#[derive(Debug, FromMeta)]
struct Args {
    runtime_path: Option<Path>,
    #[darling(default, with = "darling::util::parse_expr::preserve_str_literal", map = "Some")]
    workers: Option<Expr>,
    #[darling(default, with = "darling::util::parse_expr::preserve_str_literal", map = "Some")]
    builder: Option<Expr>,
}

impl Args {
    fn runtime(&self, runtime_path: &Path, clock: Option<&Ident>) -> syn::Result<TokenStream> {
        if let Some(builder) = &self.builder {
            if self.workers.is_some() {
                return Err(syn::Error::new_spanned(builder, "`builder` cannot be combined with `workers`"));
            }
            if clock.is_some() {
                return Err(syn::Error::new_spanned(
                    builder,
                    "`builder` cannot be combined with a ClockControl argument; configure the clock explicitly with RuntimeBuilder",
                ));
            }
            return Ok(quote!(#runtime_path::RuntimeBuilder::build(#builder)));
        }
        if self.workers.is_none() && clock.is_none() {
            return Ok(quote!(#runtime_path::Runtime::new()));
        }
        let workers = self.workers.as_ref().map(worker_count).transpose()?;
        let workers = workers.map(|count| {
            quote! {
                .processor_count(#runtime_path::ProcessorCount::at_most(
                    const {
                        ::core::num::NonZero::new(#count)
                            .expect("workers was validated as a nonzero literal by the macro")
                    }
                ))
            }
        });
        let clock = clock.map(|binding| quote!(.clock(::core::clone::Clone::clone(&#binding))));
        Ok(quote!(#runtime_path::Runtime::builder() #workers #clock .build()))
    }
}

fn worker_count(mut value: &Expr) -> syn::Result<&syn::LitInt> {
    while let Expr::Group(group) = value {
        value = &group.expr;
    }
    if let Expr::Lit(syn::ExprLit { lit: Lit::Int(count), .. }) = value
        && !count.base10_digits().starts_with('-')
        && !count.base10_digits().chars().all(|digit| digit == '0')
        && matches!(count.suffix(), "" | "usize")
    {
        return Ok(count);
    }
    Err(syn::Error::new_spanned(
        value,
        "`workers` must be a nonzero integer literal, optionally suffixed with `usize`; use `builder` for expressions or processor policies",
    ))
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
    let default_path = parse_quote!(::arty::runtime);
    let runtime_path = args.runtime_path.as_ref().unwrap_or(&default_path);
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
    let state_ident = state_ident.clone();
    let state_type = state_type.clone();
    let clock = match inputs.next() {
        Some(syn::FnArg::Typed(clock)) if test && matches!(clock.ty.as_ref(), syn::Type::Path(_)) => {
            let syn::Pat::Ident(ident) = clock.pat.as_ref() else {
                return fail(syn::Error::new_spanned(&clock.pat, "argument must have an identifier"));
            };
            if ident.ident.unraw() == state_ident.ident.unraw() {
                return fail(syn::Error::new_spanned(ident, "arguments must have distinct identifiers"));
            }
            Some((ident.clone(), clock.ty.clone()))
        }
        Some(extra) => return fail(syn::Error::new_spanned(extra, "unexpected arguments")),
        None => None,
    };
    if let Some(extra) = inputs.next() {
        return fail(syn::Error::new_spanned(extra, "unexpected arguments"));
    }
    let clock_binding = Ident::new("__arty_clock_control", Span::mixed_site());
    let runtime = match args.runtime(runtime_path, clock.as_ref().map(|_| &clock_binding)) {
        Ok(runtime) => runtime,
        Err(error) => return fail(error),
    };
    sig.asyncness = None;
    sig.inputs.clear();
    let mut attrs = input.attrs;
    if test {
        attrs.push(parse_quote!(#[::core::prelude::v1::test]));
    }
    let visibility = input.vis;
    let body = input.block;
    let (setup, body) = if let Some((ident, ty)) = clock {
        (
            quote!(let #clock_binding = #runtime_path::__private::ClockControl::new();),
            quote!({
                let #ident: #ty = #clock_binding;
                #body
            }),
        )
    } else {
        (TokenStream::new(), quote!(#body))
    };
    quote! {
        #(#attrs)*
        #visibility #sig {
            #setup
            #runtime
                .expect("failed to create the runtime for the entry point")
                .run(async move |#state_ident: #state_type| #body)
                .unwrap_or_else(|error| #runtime_path::__private::resume_join_error(error))
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
                .unwrap_or_else(|error| ::arty::runtime::__private::resume_join_error(error))
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
                .unwrap_or_else(|error| ::renamed::__private::resume_join_error(error))
        }
        "#);
    }

    #[test]
    fn workers_selects_at_most_the_literal_count() {
        let expansion = main(
            quote!(workers = 4usize, runtime_path = ::renamed),
            quote! {
                pub async fn run(cx: <App as Types>::Context) -> AppResult {
                    run(cx).await
                }
            },
        );
        let expected = quote! {
            pub fn run() -> AppResult {
                ::renamed::Runtime::builder()
                    .processor_count(::renamed::ProcessorCount::at_most(
                        const {
                            ::core::num::NonZero::new(4usize)
                                .expect("workers was validated as a nonzero literal by the macro")
                        }
                    ))
                    .build()
                    .expect("failed to create the runtime for the entry point")
                    .run(async move |cx: <App as Types>::Context| { run(cx).await })
                    .unwrap_or_else(|error| ::renamed::__private::resume_join_error(error))
            }
        };
        assert_eq!(expansion.to_string(), expected.to_string());
    }

    #[test]
    fn builder_expression_preserves_explicit_question_mark_and_result_alias() {
        let expansion = main(
            quote!(builder = app_builder()?, runtime_path = ::renamed),
            quote! {
                async fn run(cx: Context) -> AppResult {
                    run(cx).await
                }
            },
        );
        let expected = quote! {
            fn run() -> AppResult {
                ::renamed::RuntimeBuilder::build(app_builder()?)
                    .expect("failed to create the runtime for the entry point")
                    .run(async move |cx: Context| { run(cx).await })
                    .unwrap_or_else(|error| ::renamed::__private::resume_join_error(error))
            }
        };
        assert_eq!(expansion.to_string(), expected.to_string());
    }

    #[test]
    fn controlled_test_shares_one_clock_and_preserves_qualified_types() {
        let expansion = test(
            quote!(workers = 1, runtime_path = crate::renamed),
            quote! {
                #[ignore = "manual test"]
                #[should_panic(expected = "original payload")]
                async fn run(cx: <App as Types>::Context, mut time: <App as Types>::Control) {
                    run(cx, &mut time).await;
                }
            },
        );
        let expected = quote! {
            #[ignore = "manual test"]
            #[should_panic(expected = "original payload")]
            #[::core::prelude::v1::test]
            fn run() {
                let __arty_clock_control = crate::renamed::__private::ClockControl::new();
                crate::renamed::Runtime::builder()
                    .processor_count(crate::renamed::ProcessorCount::at_most(
                        const {
                            ::core::num::NonZero::new(1)
                                .expect("workers was validated as a nonzero literal by the macro")
                        }
                    ))
                    .clock(::core::clone::Clone::clone(&__arty_clock_control))
                    .build()
                    .expect("failed to create the runtime for the entry point")
                    .run(async move |cx: <App as Types>::Context| {
                        let mut time: <App as Types>::Control = __arty_clock_control;
                        {
                            run(cx, &mut time).await;
                        }
                    })
                    .unwrap_or_else(|error| crate::renamed::__private::resume_join_error(error))
            }
        };
        assert_eq!(expansion.to_string(), expected.to_string());
    }

    #[test]
    fn controlled_test_without_workers_keeps_default_processor_selection() {
        let expansion = test(
            TokenStream::new(),
            quote! {
                async fn run(cx: Builtins, time: ClockControl) {
                    run(cx, time).await;
                }
            },
        );
        let expansion = expansion.to_string();
        assert!(expansion.contains(":: arty :: runtime :: Runtime :: builder"));
        assert!(expansion.contains(":: arty :: runtime :: __private :: ClockControl :: new"));
        assert!(!expansion.contains("processor_count"));
    }

    #[test]
    fn worker_literals_are_not_limited_to_the_macro_hosts_pointer_width() {
        let forwarded = proc_macro2::TokenTree::Group(proc_macro2::Group::new(proc_macro2::Delimiter::None, quote!(2)));
        for workers in [
            quote!(1),
            quote!(0x10usize),
            quote!(340282366920938463463374607431768211456),
            TokenStream::from(forwarded),
        ] {
            let expansion = main(
                quote!(workers = #workers),
                quote! {
                    async fn run(cx: Builtins) {}
                },
            );
            assert!(!expansion.to_string().contains("compile_error"));
        }
    }

    #[test]
    fn builder_strings_remain_literals_for_type_checking() {
        let expansion = main(
            quote!(builder = "app_builder()"),
            quote! {
                async fn run(cx: Builtins) {}
            },
        );
        assert!(expansion.to_string().contains("RuntimeBuilder :: build (\"app_builder()\")"));
    }

    #[test]
    fn invalid_worker_counts_report_errors() {
        for workers in [
            quote!(0),
            quote!(0x0usize),
            quote!(-0),
            quote!(-1),
            quote!(-1usize),
            quote!(-0x10),
            quote!(1u32),
            quote!(1.5),
            quote!("2"),
            quote!(true),
            quote!(COUNT),
            quote!(1 + 1),
            quote!(ProcessorCount::all()),
        ] {
            for expand in [main, test] {
                let expansion = expand(
                    quote!(workers = #workers),
                    quote! {
                        async fn run(cx: Builtins) {}
                    },
                )
                .to_string();
                assert!(expansion.contains("compile_error"), "workers = {workers}: {expansion}");
                assert!(expansion.contains("nonzero integer literal"), "{expansion}");
            }
        }
    }

    #[test]
    fn conflicting_configuration_is_rejected_in_either_order() {
        for args in [
            quote!(builder = configure(), workers = 1),
            quote!(workers = 1, builder = configure()),
        ] {
            for expand in [main, test] {
                let expansion = expand(
                    args.clone(),
                    quote! {
                        async fn run(cx: Builtins) {}
                    },
                )
                .to_string();
                assert!(expansion.contains("`builder` cannot be combined with `workers`"));
            }
        }
        let expansion = test(
            quote!(builder = configure()),
            quote! {
                async fn run(cx: Builtins, control: ClockControl) {}
            },
        )
        .to_string();
        assert!(expansion.contains("`builder` cannot be combined with a ClockControl argument"));
    }

    #[test]
    fn injected_clock_requires_a_named_owned_second_test_parameter() {
        for input in [
            quote!(
                async fn run(cx: Builtins, _: ClockControl) {}
            ),
            quote!(
                async fn run(cx: Builtins, control: &ClockControl) {}
            ),
            quote!(
                async fn run(cx: Builtins, control: ClockControl, extra: ClockControl) {}
            ),
            quote!(
                async fn run(cx: Builtins, cx: ClockControl) {}
            ),
            quote!(
                async fn run(cx: Builtins, r#cx: ClockControl) {}
            ),
        ] {
            for expand in [main, test] {
                let expansion = expand(TokenStream::new(), input.clone()).to_string();
                assert!(expansion.starts_with(&input.to_string()));
                assert!(expansion.contains("compile_error"), "{expansion}");
            }
        }
        let expansion = main(
            TokenStream::new(),
            quote!(
                async fn run(cx: Builtins, control: ClockControl) {}
            ),
        )
        .to_string();
        assert!(expansion.contains("unexpected arguments"));
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
            (
                quote!(workers = 1, workers = 2),
                quote!(
                    async fn run(cx: Builtins) {}
                ),
            ),
            (
                quote!(builder = configure(), builder = configure()),
                quote!(
                    async fn run(cx: Builtins) {}
                ),
            ),
            (
                quote!(builder =),
                quote!(
                    async fn run(cx: Builtins) {}
                ),
            ),
        ] {
            assert!(main(args, input).to_string().contains("compile_error"));
        }
    }
}
