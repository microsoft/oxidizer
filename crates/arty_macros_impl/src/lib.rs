// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![forbid(unsafe_code)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Token expansion for Arty's entry-point attributes.
//!
//! This companion crate provides the expansion functions used by
//! [`arty_macros`](https://docs.rs/arty_macros). Application code should enable
//! Arty's `macros` feature and use its `main` and `test` attributes instead.
//!
//! [`main()`] expands an asynchronous entry point; [`test()`] additionally registers
//! the function with Rust's test harness. Invalid input produces compiler
//! diagnostic tokens rather than a runtime error.

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
    fn runtime(&self, runtime_path: &Path, clock: Option<&Ident>, test: bool) -> syn::Result<TokenStream> {
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
        if self.workers.is_none() && clock.is_none() && !test {
            return Ok(quote!(#runtime_path::Runtime::new()));
        }
        let workers = self.workers.as_ref().map(worker_count).transpose()?;
        let workers = workers.map(|count| quote!(#count)).or_else(|| test.then(|| quote!(1)));
        let workers = workers.map(|count| quote!(.processor_count(#runtime_path::ProcessorCount::at_most(#count))));
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
        && matches!(count.suffix(), "" | "usize")
    {
        return Ok(count);
    }
    Err(syn::Error::new_spanned(
        value,
        "`workers` must be a nonnegative integer literal, optionally suffixed with `usize`; use `builder` for expressions or processor policies",
    ))
}

/// Expands an asynchronous entry-point function into synchronous runtime setup.
///
/// `args` contains the attribute's configuration tokens, and `item` contains the
/// annotated asynchronous function. The returned tokens define the entry point
/// or report invalid syntax with compiler diagnostics.
///
/// This is the implementation hook for `arty_macros::main`, not an application
/// entry point. See [`arty::main`](https://docs.rs/arty/latest/arty/attr.main.html)
/// for the supported configuration and generated function's behavior.
///
/// # Examples
///
/// ```
/// use quote::quote;
///
/// let expanded = arty_macros_impl::main(
///     quote!(),
///     quote!(
///         async fn main(cx: arty::task::Builtins) {}
///     ),
/// );
/// assert!(!expanded.is_empty());
/// ```
#[must_use]
pub fn main(args: TokenStream, item: TokenStream) -> TokenStream {
    entrypoint(args, item, false)
}

/// Expands an asynchronous function into a runtime-backed synchronous test.
///
/// `args` contains the attribute's configuration tokens, and `item` contains the
/// annotated asynchronous function. The returned tokens register a test with
/// Rust's test harness or report invalid syntax with compiler diagnostics.
///
/// This is the implementation hook for `arty_macros::test`. See
/// [`arty::test`](https://docs.rs/arty/latest/arty/attr.test.html) for application
/// examples and the optional controlled-time argument.
///
/// # Examples
///
/// ```
/// use quote::quote;
///
/// let expanded = arty_macros_impl::test(
///     quote!(),
///     quote!(
///         async fn checks_answer(cx: arty::task::Builtins) {
///             assert_eq!(cx.scheduler().spawn(async |_| 42).await.unwrap(), 42);
///         }
///     ),
/// );
/// assert!(!expanded.is_empty());
/// ```
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
    let runtime_binding = Ident::new("__arty_runtime", Span::mixed_site());
    let result_binding = Ident::new("__arty_result", Span::mixed_site());
    let shutdown_binding = Ident::new("__arty_shutdown", Span::mixed_site());
    let value_binding = Ident::new("__arty_value", Span::mixed_site());
    let runtime = match args.runtime(runtime_path, clock.as_ref().map(|_| &clock_binding), test) {
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
            let #runtime_binding = #runtime
                .expect("failed to create the runtime for the entry point");
            let #result_binding = #runtime_binding
                .scheduler()
                .block_on(async move |#state_ident: #state_type| #body);
            let #shutdown_binding = #runtime_binding.stop();
            #result_binding
                .and_then(|#value_binding| #shutdown_binding.map(|()| #value_binding))
                .unwrap_or_else(|error| #runtime_path::__private::resume_error(error))
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
                pub async fn main(cx: arty::task::Builtins) -> Result<(), Error> {
                    run(cx).await
                }
            },
        );
        assert_snapshot!(render_expansion(&expansion), @r#"
        pub fn main() -> Result<(), Error> {
            let __arty_runtime = ::arty::runtime::Runtime::new()
                .expect("failed to create the runtime for the entry point");
            let __arty_result = __arty_runtime
                .scheduler()
                .block_on(async move |cx: arty::task::Builtins| { run(cx).await });
            let __arty_shutdown = __arty_runtime.stop();
            __arty_result
                .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                .unwrap_or_else(|error| ::arty::runtime::__private::resume_error(error))
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
            let __arty_runtime = ::renamed::Runtime::builder()
                .processor_count(::renamed::ProcessorCount::at_most(1))
                .build()
                .expect("failed to create the runtime for the entry point");
            let __arty_result = __arty_runtime
                .scheduler()
                .block_on(async move |mut cx: renamed::Builtins| {
                    fail(&mut cx).await;
                });
            let __arty_shutdown = __arty_runtime.stop();
            __arty_result
                .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                .unwrap_or_else(|error| ::renamed::__private::resume_error(error))
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
                let __arty_runtime = ::renamed::Runtime::builder()
                    .processor_count(::renamed::ProcessorCount::at_most(4usize))
                    .build()
                    .expect("failed to create the runtime for the entry point");
                let __arty_result = __arty_runtime
                    .scheduler()
                    .block_on(async move |cx: <App as Types>::Context| { run(cx).await });
                let __arty_shutdown = __arty_runtime.stop();
                __arty_result
                    .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                    .unwrap_or_else(|error| ::renamed::__private::resume_error(error))
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
                let __arty_runtime = ::renamed::RuntimeBuilder::build(app_builder()?)
                    .expect("failed to create the runtime for the entry point");
                let __arty_result = __arty_runtime
                    .scheduler()
                    .block_on(async move |cx: Context| { run(cx).await });
                let __arty_shutdown = __arty_runtime.stop();
                __arty_result
                    .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                    .unwrap_or_else(|error| ::renamed::__private::resume_error(error))
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
                let __arty_runtime = crate::renamed::Runtime::builder()
                    .processor_count(crate::renamed::ProcessorCount::at_most(1))
                    .clock(::core::clone::Clone::clone(&__arty_clock_control))
                    .build()
                    .expect("failed to create the runtime for the entry point");
                let __arty_result = __arty_runtime
                    .scheduler()
                    .block_on(async move |cx: <App as Types>::Context| {
                        let mut time: <App as Types>::Control = __arty_clock_control;
                        {
                            run(cx, &mut time).await;
                        }
                    });
                let __arty_shutdown = __arty_runtime.stop();
                __arty_result
                    .and_then(|__arty_value| __arty_shutdown.map(|()| __arty_value))
                    .unwrap_or_else(|error| crate::renamed::__private::resume_error(error))
            }
        };
        assert_eq!(expansion.to_string(), expected.to_string());
    }

    #[test]
    fn controlled_test_without_workers_defaults_to_one_processor() {
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
        assert!(expansion.contains("processor_count"));
        assert!(expansion.contains("ProcessorCount :: at_most (1)"));
    }

    #[test]
    fn worker_literals_are_not_limited_to_the_macro_hosts_pointer_width() {
        let forwarded = proc_macro2::TokenTree::Group(proc_macro2::Group::new(proc_macro2::Delimiter::None, quote!(2)));
        for workers in [
            quote!(0),
            quote!(0x0usize),
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
    fn worker_count_unwraps_nested_expression_groups() {
        let mut value: Expr = parse_quote!(4usize);
        for _ in 0..2 {
            value = Expr::Group(syn::ExprGroup {
                attrs: Vec::new(),
                group_token: syn::token::Group::default(),
                expr: Box::new(value),
            });
        }

        let count = worker_count(&value).unwrap();
        assert_eq!(count.base10_digits(), "4");
        assert_eq!(count.suffix(), "usize");
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
                assert!(expansion.contains("nonnegative integer literal"), "{expansion}");
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
