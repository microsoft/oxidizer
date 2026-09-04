// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![forbid(unsafe_code)]
#![doc(
    html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench_macros_impl/logo.png"
)]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/metabench_macros_impl/favicon.ico"
)]

//! Implementation of the procedural macros for the [`metabench`](https://docs.rs/metabench) crate.
//!
//! This crate holds the implementation of `#[benchmarks]` and `#[benchmark]`.
//!
//! **Do not depend on this crate directly.** Use the re-exports from `metabench` instead.

use std::collections::HashSet;

use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Error, Expr, FnArg, Ident, ImplItem, ImplItemFn, ItemImpl, LitStr, Meta, Result, Token, Type, TypeReference};
use syn2 as syn;

struct BenchmarkArguments {
    name: Option<LitStr>,
    engines: Option<Expr>,
}

impl Parse for BenchmarkArguments {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut name = None;
        let mut engines = None;
        while !input.is_empty() {
            let key = input.parse::<Ident>()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "name" if name.is_none() => name = Some(input.parse()?),
                "engines" if engines.is_none() => engines = Some(input.parse()?),
                "name" | "engines" => {
                    return Err(Error::new(key.span(), "duplicate benchmark argument"));
                }
                _ => {
                    return Err(Error::new(key.span(), "expected `name` or `engines`"));
                }
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Self { name, engines })
    }
}

/// Rejects a `#[metabench::benchmark]` attribute that was not consumed by
/// `#[metabench::benchmarks]`.
///
/// # Errors
///
/// Always returns a placement error.
pub fn benchmark(input: TokenStream) -> Result<TokenStream> {
    Err(Error::new_spanned(
        input,
        "`benchmark` may only be used on a method inside a `benchmarks` impl",
    ))
}

/// Expands `#[metabench::benchmarks]`.
///
/// # Errors
///
/// Returns an error when the arguments, impl block, or benchmark method
/// signatures do not conform to the metabench fixture API.
pub fn benchmarks(arguments: TokenStream, input: TokenStream) -> Result<TokenStream> {
    let arguments = syn::parse2::<BenchmarkArguments>(arguments)?;
    let mut implementation = syn::parse2::<ItemImpl>(input)?;
    validate_impl(&implementation)?;

    let self_type = implementation.self_ty.as_ref();
    let group_name = arguments.name.map_or_else(
        || inferred_group_name(self_type).map(|name| LitStr::new(&name, proc_macro2::Span::call_site())),
        Ok,
    )?;
    if group_name.value().is_empty() || group_name.value().contains('/') {
        return Err(Error::new_spanned(
            &group_name,
            "benchmark group names must be non-empty and cannot contain `/`",
        ));
    }
    let engines = arguments
        .engines
        .unwrap_or_else(|| syn::parse_quote!(::metabench::Engines::DEFAULT));
    let mut registrations = Vec::new();
    let mut benchmark_names = HashSet::new();
    for item in &mut implementation.items {
        let ImplItem::Fn(method) = item else {
            return Err(Error::new_spanned(item, "benchmark impl blocks may contain only methods"));
        };
        let Some(method_arguments) = take_benchmark_arguments(&mut method.attrs)? else {
            continue;
        };
        validate_method(method)?;
        let benchmark_name = method_arguments
            .name
            .unwrap_or_else(|| LitStr::new(&method.sig.ident.to_string(), method.sig.ident.span()));
        if benchmark_name.value().is_empty() || benchmark_name.value().contains('/') {
            return Err(Error::new_spanned(
                &benchmark_name,
                "benchmark names must be non-empty and cannot contain `/`",
            ));
        }
        if !benchmark_names.insert(benchmark_name.value()) {
            return Err(Error::new_spanned(&benchmark_name, "duplicate benchmark name"));
        }
        if let Some(attribute) = method.attrs.iter().find(|attribute| attribute.path().is_ident("inline")) {
            return Err(Error::new_spanned(
                attribute,
                "metabench controls benchmark inlining; remove this attribute",
            ));
        }
        method.attrs.push(syn::parse_quote!(#[inline(never)]));
        let method_engines = method_arguments.engines.as_ref().unwrap_or(&engines);
        registrations.push(registration(self_type, method, &benchmark_name, method_engines));
    }
    if registrations.is_empty() {
        return Err(Error::new_spanned(
            &implementation,
            "benchmark impl blocks must contain at least one method marked with `#[benchmark]`",
        ));
    }

    Ok(quote! {
        #implementation

        impl ::metabench::__private::BenchmarkGroupDefinition for #self_type {
            fn register(__metabench_suite: &mut ::metabench::__private::BenchmarkSuite) {
                let __metabench_group =
                    __metabench_suite.benchmark_group(#group_name);
                #(#registrations)*
            }
        }
    })
}

fn take_benchmark_arguments(attributes: &mut Vec<Attribute>) -> Result<Option<BenchmarkArguments>> {
    let matching = attributes
        .iter()
        .enumerate()
        .filter(|(_, attribute)| is_benchmark_attribute(attribute))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if matching.len() > 1 {
        return Err(Error::new_spanned(&attributes[matching[1]], "duplicate `benchmark` attribute"));
    }
    let Some(index) = matching.first().copied() else {
        return Ok(None);
    };
    let attribute = attributes.remove(index);
    if matches!(&attribute.meta, Meta::Path(_)) {
        return Ok(Some(BenchmarkArguments { name: None, engines: None }));
    }
    attribute.parse_args().map(Some)
}

fn is_benchmark_attribute(attribute: &Attribute) -> bool {
    let path = attribute.path();
    path.is_ident("benchmark")
        || (path.segments.len() == 2
            && path.segments[1].ident == "benchmark"
            && (path.segments[0].ident == "metabench" || path.segments[0].ident == "metabench_macros"))
}

fn validate_impl(implementation: &ItemImpl) -> Result<()> {
    if implementation.trait_.is_some() {
        return Err(Error::new_spanned(implementation, "`benchmarks` requires an inherent impl"));
    }
    if !implementation.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &implementation.generics,
            "generic benchmark impl blocks are not supported",
        ));
    }
    Ok(())
}

fn validate_method(method: &ImplItemFn) -> Result<()> {
    if method.sig.asyncness.is_some()
        || method.sig.constness.is_some()
        || method.sig.unsafety.is_some()
        || !method.sig.generics.params.is_empty()
    {
        return Err(Error::new_spanned(
            &method.sig,
            "benchmark methods must be synchronous, safe, non-const, and non-generic",
        ));
    }
    let mut inputs = method.sig.inputs.iter();
    match inputs.next() {
        None => {}
        Some(FnArg::Receiver(receiver)) if receiver.reference.is_some() && inputs.next().is_none() => {}
        Some(FnArg::Typed(argument))
            if matches!(argument.ty.as_ref(), Type::Reference(TypeReference { mutability: None, .. })) && inputs.next().is_none() => {}
        _ => {
            return Err(Error::new_spanned(
                &method.sig.inputs,
                "benchmark methods must take no arguments, one shared case reference, `&self`, or `&mut self`",
            ));
        }
    }
    Ok(())
}

fn registration(self_type: &Type, method: &ImplItemFn, benchmark_name: &LitStr, engines: &Expr) -> TokenStream {
    let method_name = &method.sig.ident;
    let Some(first_input) = method.sig.inputs.first() else {
        return quote! {
            __metabench_group.benchmark_case(
                #benchmark_name,
                None,
                #engines,
                |__metabench_bencher| {
                    __metabench_bencher.run(|| #self_type::#method_name());
                },
            );
        };
    };
    let FnArg::Receiver(receiver) = first_input else {
        return quote! {
            for __metabench_case in
                <#self_type as ::metabench::BenchmarkCases>::cases()
            {
                let __metabench_case_name =
                    ::metabench::BenchmarkCase::name(&__metabench_case);
                let __metabench_case_name = if __metabench_case_name.is_empty() {
                    None
                } else {
                    Some(__metabench_case_name)
                };
                __metabench_group.benchmark_case(
                    #benchmark_name,
                    __metabench_case_name,
                    #engines,
                    move |__metabench_bencher| {
                        __metabench_bencher.run(
                            || #self_type::#method_name(&__metabench_case)
                        );
                    },
                );
            }
        };
    };
    let (fixture_binding, invocation) = if receiver.mutability.is_some() {
        (
            quote!(mut __metabench_fixture),
            quote!(#self_type::#method_name(&mut __metabench_fixture)),
        )
    } else {
        (quote!(__metabench_fixture), quote!(#self_type::#method_name(&__metabench_fixture)))
    };
    quote! {
        for __metabench_case in
            <#self_type as ::metabench::Fixture>::cases()
        {
            let __metabench_case_name =
                ::metabench::BenchmarkCase::name(&__metabench_case);
            let __metabench_case_name = if __metabench_case_name.is_empty() {
                None
            } else {
                Some(__metabench_case_name)
            };
            __metabench_group.benchmark_case(
                #benchmark_name,
                __metabench_case_name,
                #engines,
                move |__metabench_bencher| {
                    __metabench_bencher
                        .setup(|| {
                            <#self_type as ::metabench::Fixture>::setup(
                                &__metabench_case,
                            )
                        })
                        .run(|#fixture_binding| {
                            let __metabench_output = #invocation;
                            ::metabench::__private::PreparedOutput::new(
                                __metabench_output,
                                __metabench_fixture,
                            )
                        });
                },
            );
        }
    }
}

fn inferred_group_name(self_type: &Type) -> Result<String> {
    let Type::Path(path) = self_type else {
        return Err(Error::new_spanned(self_type, "benchmark group type must be a path"));
    };
    let identifier = &path
        .path
        .segments
        .last()
        .ok_or_else(|| Error::new_spanned(self_type, "missing benchmark group type"))?
        .ident;
    let type_name = identifier.to_string();
    let base = type_name.strip_suffix("Benchmarks").unwrap_or(&type_name);
    Ok(base.to_snake_case())
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;

    fn assert_benchmarks_error(arguments: TokenStream, input: TokenStream, expected: &str) {
        let error = benchmarks(arguments, input).expect_err("invalid benchmark declaration");

        assert!(error.to_string().contains(expected));
    }

    #[test]
    fn expands_stateful_and_stateless_methods() {
        let output = benchmarks(
            quote!(engines = Engines::ALL),
            quote! {
                impl HashMapBenchmarks {
                    #[benchmark]
                    fn insert(&mut self) {}
                    #[benchmark]
                    fn capacity() -> usize { 0 }
                }
            },
        )
        .expect("valid benchmark impl")
        .to_string();

        assert!(output.contains("BenchmarkGroupDefinition"));
        assert!(output.contains("\"hash_map\""));
        assert!(output.contains("\"insert\""));
        assert!(output.contains("\"capacity\""));
    }

    #[test]
    fn honors_explicit_group_name() {
        let output = benchmarks(
            quote!(name = "maps", engines = Engines::CRITERION),
            quote! {
                impl HashMapBenchmarks {
                    #[benchmark]
                    fn capacity(&self) -> usize { 0 }
                }
            },
        )
        .expect("valid benchmark impl")
        .to_string();

        assert!(output.contains("\"maps\""));
    }

    #[test]
    fn uses_default_engines_when_omitted() {
        let output = benchmarks(quote!(), quote!(impl TestBenchmarks { #[benchmark] fn work() {} }))
            .expect("engines are optional")
            .to_string();

        assert!(output.contains("Engines :: DEFAULT"));
    }

    #[test]
    fn rejects_by_value_receiver() {
        let error = benchmarks(
            quote!(engines = Engines::ALL),
            quote!(impl TestBenchmarks { #[benchmark] fn work(self) {} }),
        )
        .expect_err("by-value receivers are unsupported");

        assert!(error.to_string().contains("`&self`, or `&mut self`"));
    }

    #[test]
    fn rejects_receiverless_parameters() {
        let error = benchmarks(
            quote!(engines = Engines::ALL),
            quote!(impl TestBenchmarks { #[benchmark] fn work(value: usize, other: usize) {} }),
        )
        .expect_err("multiple receiverless parameters are unsupported");

        assert!(error.to_string().contains("one shared case reference"));
    }

    #[test]
    fn expands_stateless_data_driven_method() {
        let output = benchmarks(
            quote!(engines = Engines::ALL),
            quote!(impl ParsingBenchmarks { #[benchmark] fn parse(case: &ParsingCase) {} }),
        )
        .expect("shared case references are supported")
        .to_string();

        assert!(output.contains("BenchmarkCases"));
        assert!(output.contains("__metabench_case"));
    }

    #[test]
    fn rejects_mutable_case_reference() {
        let error = benchmarks(
            quote!(engines = Engines::ALL),
            quote!(impl ParsingBenchmarks { #[benchmark] fn parse(case: &mut ParsingCase) {} }),
        )
        .expect_err("cases must not be mutated between measurements");

        assert!(error.to_string().contains("one shared case reference"));
    }

    #[test]
    fn rejects_associated_items() {
        let error = benchmarks(
            quote!(engines = Engines::ALL),
            quote!(impl TestBenchmarks { const VALUE: usize = 1; }),
        )
        .expect_err("associated constants are unsupported");

        assert!(error.to_string().contains("only methods"));
    }

    #[test]
    fn rejects_inline_attributes() {
        let error = benchmarks(
            quote!(engines = Engines::ALL),
            quote! {
                impl TestBenchmarks {
                    #[inline(always)]
                    #[benchmark]
                    fn work() {}
                }
            },
        )
        .expect_err("metabench controls inlining");

        assert!(error.to_string().contains("controls benchmark inlining"));
    }

    #[test]
    fn rejects_invalid_explicit_group_name() {
        let error = benchmarks(
            quote!(name = "maps/read", engines = Engines::ALL),
            quote!(impl MapBenchmarks { fn work() {} }),
        )
        .expect_err("slashes make workload identities ambiguous");

        assert!(error.to_string().contains("cannot contain `/`"));
    }

    #[test]
    fn rejects_every_invalid_macro_shape() {
        let cases = [
            (
                quote!(name = "first", name = "second"),
                quote!(impl TestBenchmarks { fn work() {} }),
                "duplicate benchmark argument",
            ),
            (
                quote!(unknown = "value"),
                quote!(impl TestBenchmarks { fn work() {} }),
                "expected `name` or `engines`",
            ),
            (quote!(), quote!(impl Benchmarks { fn work() {} }), "group names must be non-empty"),
            (
                quote!(),
                quote! {
                    impl TestBenchmarks {
                        #[benchmark(name = "invalid/name")]
                        fn work() {}
                    }
                },
                "benchmark names must be non-empty",
            ),
            (
                quote!(),
                quote! {
                    impl TestBenchmarks {
                        #[benchmark(name = "same")]
                        fn first() {}
                        #[benchmark(name = "same")]
                        fn second() {}
                    }
                },
                "duplicate benchmark name",
            ),
            (
                quote!(),
                quote! {
                    impl TestBenchmarks {
                        #[benchmark]
                        #[benchmark]
                        fn work() {}
                    }
                },
                "duplicate `benchmark` attribute",
            ),
            (
                quote!(),
                quote!(impl TestBenchmarks {}),
                "at least one method marked with `#[benchmark]`",
            ),
            (
                quote!(),
                quote!(impl TestBenchmarks { fn helper() {} }),
                "at least one method marked with `#[benchmark]`",
            ),
            (
                quote!(),
                quote!(impl Work for TestBenchmarks { fn work() {} }),
                "requires an inherent impl",
            ),
            (
                quote!(),
                quote!(
                    impl<T> TestBenchmarks<T> {
                        fn work() {}
                    }
                ),
                "generic benchmark impl blocks",
            ),
            (quote!(), quote!(impl [u8; 4] { fn work() {} }), "group type must be a path"),
            (
                quote!(),
                quote!(impl TestBenchmarks { #[benchmark] async fn work() {} }),
                "synchronous, safe, non-const, and non-generic",
            ),
            (
                quote!(),
                quote!(impl TestBenchmarks { #[benchmark] const fn work() {} }),
                "synchronous, safe, non-const, and non-generic",
            ),
            (
                quote!(),
                quote!(impl TestBenchmarks { #[benchmark] unsafe fn work() {} }),
                "synchronous, safe, non-const, and non-generic",
            ),
            (
                quote!(),
                quote!(impl TestBenchmarks { #[benchmark] fn work<T>() {} }),
                "synchronous, safe, non-const, and non-generic",
            ),
        ];

        for (arguments, input, expected) in cases {
            assert_benchmarks_error(arguments, input, expected);
        }
    }

    #[test]
    fn expands_method_specific_arguments() {
        let output = benchmarks(
            quote!(engines = Engines::ALL),
            quote! {
                impl TestBenchmarks {
                    #[benchmark(name = "renamed", engines = Engines::PERF)]
                    fn work() {}
                }
            },
        )
        .expect("method-specific arguments are valid")
        .to_string();

        assert!(output.contains("\"renamed\""));
        assert!(output.contains("Engines :: PERF"));
    }

    #[test]
    fn preserves_unrelated_qualified_benchmark_attributes() {
        let output = benchmarks(
            quote!(),
            quote! {
                impl TestBenchmarks {
                    #[other_crate::benchmark]
                    #[metabench::benchmark(name = "renamed")]
                    fn work() {}
                }
            },
        )
        .expect("unrelated qualified attributes are preserved")
        .to_string();

        assert!(output.contains("other_crate :: benchmark"));
        assert!(output.contains("\"renamed\""));
    }

    #[test]
    fn rejects_malformed_inputs_and_misplaced_benchmark_attribute() {
        assert_benchmarks_error(quote!(name), quote!(impl TestBenchmarks { fn work() {} }), "expected `=`");
        assert_benchmarks_error(
            quote!(),
            quote!(
                fn misplaced() {}
            ),
            "expected `impl`",
        );
        assert_benchmarks_error(
            quote!(),
            quote! {
                impl TestBenchmarks {
                    #[benchmark(name)]
                    fn work() {}
                }
            },
            "expected `=`",
        );

        let error = benchmark(quote!(
            fn misplaced() {}
        ))
        .expect_err("the attribute requires a benchmark method");
        assert!(error.to_string().contains("may only be used on a method"));
    }
}
