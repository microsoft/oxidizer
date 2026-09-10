// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote, quote_spanned};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Error, Expr, ExprLit, FnArg, GenericParam, Ident, ItemFn, Lit, LitStr, Meta, MetaList, MetaNameValue, Result, Safety,
    Signature, Token, parse_quote, parse2,
};

use crate::shared::{metabench_path, parse_ident_value, reject_if_too_complex, support_ident};

struct BenchmarkArguments {
    identity: Ident,
    group_name: Expr,
    benchmark_name: LitStr,
    native: Punctuated<MetaNameValue, Token![,]>,
}

impl Parse for BenchmarkArguments {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let lookahead = input.fork();
        let named = lookahead.parse::<Ident>().is_ok() && lookahead.peek(Token![=]);
        if named {
            Self::parse_named(input)
        } else {
            Self::parse_positional(input)
        }
    }
}

impl BenchmarkArguments {
    fn parse_named(input: ParseStream<'_>) -> Result<Self> {
        let arguments = Punctuated::<MetaNameValue, Token![,]>::parse_terminated(input)?;
        let mut identity = None;
        let mut group_name = None;
        let mut benchmark_name = None;
        let mut native = Punctuated::new();

        for argument in arguments {
            if argument.path.is_ident("identity") {
                if identity.is_some() {
                    return Err(Error::new_spanned(argument, "`identity` may only be specified once"));
                }
                identity = Some(parse_ident_value(&argument.value, "identity")?);
            } else if argument.path.is_ident("group_name") {
                if group_name.is_some() {
                    return Err(Error::new_spanned(argument, "`group_name` may only be specified once"));
                }
                group_name = Some(argument.value);
            } else if argument.path.is_ident("benchmark_name") {
                if benchmark_name.is_some() {
                    return Err(Error::new_spanned(argument, "`benchmark_name` may only be specified once"));
                }
                benchmark_name = Some(parse_string_value(&argument.value, "benchmark_name")?);
            } else {
                push_native_argument(&mut native, argument)?;
            }
        }

        Ok(Self {
            identity: identity.ok_or_else(|| input.error("missing `identity`"))?,
            group_name: group_name.ok_or_else(|| input.error("missing `group_name`"))?,
            benchmark_name: benchmark_name.ok_or_else(|| input.error("missing `benchmark_name`"))?,
            native,
        })
    }

    fn parse_positional(input: ParseStream<'_>) -> Result<Self> {
        let identity = input.parse()?;
        input.parse::<Token![,]>()?;
        reject_mixed_field(input, "group_name")?;
        let group_name = input.parse()?;
        input.parse::<Token![,]>()?;
        reject_mixed_field(input, "benchmark_name")?;
        let benchmark_name = input.parse()?;

        let mut native = Punctuated::new();
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if !input.is_empty() {
                for argument in Punctuated::<MetaNameValue, Token![,]>::parse_terminated(input)? {
                    push_native_argument(&mut native, argument)?;
                }
            }
        }

        Ok(Self {
            identity,
            group_name,
            benchmark_name,
            native,
        })
    }
}

fn reject_mixed_field(input: ParseStream<'_>, expected: &str) -> Result<()> {
    let lookahead = input.fork();
    if lookahead.parse::<Ident>().is_ok_and(|field| field == expected) && lookahead.peek(Token![=]) {
        Err(input.error("positional and named benchmark arguments cannot be mixed"))
    } else {
        Ok(())
    }
}

fn push_native_argument(native: &mut Punctuated<MetaNameValue, Token![,]>, argument: MetaNameValue) -> Result<()> {
    let (source_name, native_name) = if argument.path.is_ident("gungraun_config") {
        ("gungraun_config", "config")
    } else if argument.path.is_ident("gungraun_setup") {
        ("gungraun_setup", "setup")
    } else if argument.path.is_ident("gungraun_teardown") {
        ("gungraun_teardown", "teardown")
    } else {
        return Err(Error::new_spanned(argument.path, "unsupported benchmark option"));
    };
    if native.iter().any(|existing| existing.path.is_ident(native_name)) {
        return Err(Error::new_spanned(
            argument.path,
            format!("`{source_name}` may only be specified once"),
        ));
    }
    native.push(native_argument(argument)?);
    Ok(())
}

fn native_argument(mut argument: MetaNameValue) -> Result<MetaNameValue> {
    if argument.path.is_ident("gungraun_config") {
        argument.path = parse_quote!(config);
    } else if argument.path.is_ident("gungraun_setup") {
        argument.path = parse_quote!(setup);
    } else if argument.path.is_ident("gungraun_teardown") {
        argument.path = parse_quote!(teardown);
    } else {
        return Err(Error::new_spanned(argument.path, "unsupported benchmark option"));
    }
    Ok(argument)
}

fn parse_string_value(value: &Expr, field: &str) -> Result<LitStr> {
    let Expr::Lit(ExprLit { lit: Lit::Str(value), .. }) = value else {
        return Err(Error::new_spanned(value, format!("`{field}` must be a string literal")));
    };
    Ok(LitStr::new(&value.value(), value.span()))
}

fn is_case_meta(meta: &Meta) -> bool {
    meta.path()
        .segments
        .first()
        .is_some_and(|segment| segment.ident == "bench" || segment.ident == "benches")
}

#[derive(Default)]
struct ForwardedAttributes {
    callable: Vec<TokenStream2>,
    conditional: Vec<TokenStream2>,
    cases: Vec<TokenStream2>,
}

fn cfg_attr_parts(list: &MetaList) -> Result<(Meta, Vec<Meta>)> {
    let arguments = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    if arguments.len() < 2 {
        return Err(Error::new_spanned(
            list,
            "`cfg_attr` requires a condition and at least one attribute",
        ));
    }
    let mut arguments = arguments.into_iter();
    let condition = arguments.next().expect("length checked above to contain a cfg_attr condition");
    Ok((condition, arguments.collect()))
}

fn rebuild_cfg_attr(span: Span, condition: &Meta, attributes: &[TokenStream2]) -> TokenStream2 {
    quote_spanned!(span=> cfg_attr(#condition, #(#attributes),*))
}

fn partition_meta(meta: &Meta) -> Result<ForwardedAttributes> {
    if is_case_meta(meta) {
        return Ok(ForwardedAttributes {
            cases: vec![quote!(#meta)],
            ..ForwardedAttributes::default()
        });
    }
    if meta.path().is_ident("inline") {
        return Ok(ForwardedAttributes::default());
    }
    if meta.path().is_ident("cfg") {
        let tokens = quote!(#meta);
        return Ok(ForwardedAttributes {
            callable: vec![tokens.clone()],
            conditional: vec![tokens],
            cases: Vec::new(),
        });
    }
    if let Meta::List(list) = meta
        && list.path.is_ident("cfg_attr")
    {
        let span = list.path.segments[0].ident.span();
        let (condition, attributes) = cfg_attr_parts(list)?;
        let mut callable = Vec::new();
        let mut conditional = Vec::new();
        let mut cases = Vec::new();
        for attribute in attributes {
            let partitioned = partition_meta(&attribute)?;
            callable.extend(partitioned.callable);
            conditional.extend(partitioned.conditional);
            cases.extend(partitioned.cases);
        }
        return Ok(ForwardedAttributes {
            callable: (!callable.is_empty())
                .then(|| rebuild_cfg_attr(span, &condition, &callable))
                .into_iter()
                .collect(),
            conditional: (!conditional.is_empty())
                .then(|| rebuild_cfg_attr(span, &condition, &conditional))
                .into_iter()
                .collect(),
            cases: (!cases.is_empty())
                .then(|| rebuild_cfg_attr(span, &condition, &cases))
                .into_iter()
                .collect(),
        });
    }

    Ok(ForwardedAttributes {
        callable: vec![quote!(#meta)],
        ..ForwardedAttributes::default()
    })
}

fn partition_attributes(attributes: Vec<Attribute>) -> Result<ForwardedAttributes> {
    let mut result = ForwardedAttributes::default();
    for attribute in attributes {
        let span = attribute.pound_token.span;
        let partitioned = partition_meta(&attribute.meta)?;
        result
            .callable
            .extend(partitioned.callable.into_iter().map(|meta| quote_spanned!(span=> #[#meta])));
        result
            .conditional
            .extend(partitioned.conditional.into_iter().map(|meta| quote_spanned!(span=> #[#meta])));
        result
            .cases
            .extend(partitioned.cases.into_iter().map(|meta| quote_spanned!(span=> #[#meta])));
    }
    Ok(result)
}

fn adapter_signature(signature: &Signature, adapter_name: &Ident) -> Result<(TokenStream2, Vec<Ident>)> {
    if let Some(asyncness) = &signature.asyncness {
        return Err(Error::new_spanned(
            asyncness,
            "async benchmark functions are unsupported because Gungraun does not await returned futures",
        ));
    }
    if let Some(parameter) = signature.generics.const_params().next() {
        return Err(Error::new_spanned(
            parameter,
            "const-generic benchmark functions are unsupported because Gungraun cannot forward the adapter const parameter",
        ));
    }
    if let Some(variadic) = &signature.variadic {
        return Err(Error::new_spanned(
            variadic,
            "variadic benchmark functions are unsupported because their arguments cannot be forwarded",
        ));
    }
    if let Safety::Unsafe(unsafety) = &signature.safety {
        return Err(Error::new_spanned(
            unsafety,
            "unsafe benchmark functions are unsupported because the generated adapter cannot uphold their safety \
             contract; wrap the unsafe call in a safe function that establishes its preconditions and benchmark that instead",
        ));
    }

    let generics = &signature.generics;
    let params = &generics.params;
    let lt_token = &generics.lt_token;
    let gt_token = &generics.gt_token;
    let where_clause = &generics.where_clause;
    let output = &signature.output;
    let mut inputs = Vec::with_capacity(signature.inputs.len());
    let mut call_arguments = Vec::with_capacity(signature.inputs.len());
    for (index, argument) in signature.inputs.iter().enumerate() {
        let FnArg::Typed(argument) = argument else {
            return Err(Error::new_spanned(argument, "benchmark functions cannot have a receiver"));
        };
        let name = format_ident!("__metabench_argument_{index}", span = Span::mixed_site());
        let attrs = &argument.attrs;
        let ty = &argument.ty;
        inputs.push(quote!(#(#attrs)* #name: #ty));
        call_arguments.push(name);
    }

    Ok((
        quote!(fn #adapter_name #lt_token #params #gt_token (#(#inputs),*) #output #where_clause),
        call_arguments,
    ))
}

fn expand_benchmark(arguments: BenchmarkArguments, mut function: ItemFn, metabench: &TokenStream2) -> Result<TokenStream2> {
    let BenchmarkArguments {
        identity,
        group_name,
        benchmark_name,
        native,
    } = arguments;
    let attributes = partition_attributes(std::mem::take(&mut function.attrs))?;
    let callable_attributes = attributes.callable;
    let conditional_attributes = attributes.conditional;
    let case_attributes = attributes.cases;
    let allocation_tracking = function.sig.constness.is_none();
    if allocation_tracking {
        let body = function.block;
        let allocation_measurement = format_ident!("_metabench_allocation_measurement", span = Span::mixed_site());
        let perf_measurement = format_ident!("_metabench_perf_measurement", span = Span::mixed_site());
        function.block = parse_quote!({
            let #allocation_measurement = #metabench::__private::begin_allocation_measurement();
            let #perf_measurement = #metabench::__private::begin_perf_measurement();
            #body
        });
    }
    function.attrs.push(syn::parse_quote!(#[inline(never)]));

    let callable_name = &function.sig.ident;
    let logical_identity = identity.to_string();
    let adapter_name = support_ident("benchmark", &logical_identity);
    let native_group_name = support_ident("group", &logical_identity);
    let (adapter_signature, call_arguments) = adapter_signature(&function.sig, &adapter_name)?;
    let generic_arguments = function.sig.generics.params.iter().filter_map(|parameter| match parameter {
        GenericParam::Type(parameter) => Some(&parameter.ident),
        GenericParam::Lifetime(_) | GenericParam::Const(_) => None,
    });
    let generic_arguments = generic_arguments.collect::<Vec<_>>();
    let turbofish = (!generic_arguments.is_empty()).then(|| quote!(::<#(#generic_arguments),*>));
    let call = quote!(#callable_name #turbofish (#(#call_arguments),*));
    let native_attribute = if native.is_empty() {
        quote!(#[#metabench::__private::gungraun::library_benchmark])
    } else {
        quote!(#[#metabench::__private::gungraun::library_benchmark(#native)])
    };
    let adapter_module = support_ident("native", &logical_identity);
    let visibility = &function.vis;

    Ok(quote! {
        #(#callable_attributes)*
        #function

        #(#conditional_attributes)*
        #visibility const #identity: #metabench::BenchmarkIdentity =
            #metabench::BenchmarkIdentity::__new(
                #group_name,
                #benchmark_name,
                stringify!(#native_group_name),
                stringify!(#adapter_name),
                #allocation_tracking,
            );

        #(#conditional_attributes)*
        mod #adapter_module {
            use super::*;
            use #metabench::__private::gungraun;

            #native_attribute
            #(#case_attributes)*
            #adapter_signature {
                #call
            }
        }
    })
}

/// Defines one logical benchmark and generates its native Gungraun adapter.
///
/// The first three arguments are the generated identity, a const group-name
/// expression, and the benchmark-name string. They may instead use the named
/// `identity`, `group_name`, and `benchmark_name` form; the two forms cannot be
/// mixed.
///
/// `gungraun_config`, `gungraun_setup`, and `gungraun_teardown` configure the
/// generated native benchmark. Native `#[bench::...]` and `#[benches::...]`
/// attributes on the function are forwarded unchanged.
///
/// The generated adapter invokes the preserved function, so Criterion and
/// Gungraun execute the same workload body. The preserved function is always
/// marked `#[inline(never)]` so both engines call the same compiled workload
/// symbol; any caller-supplied `inline` attribute is replaced.
///
/// Async, const-generic, and unsafe functions are rejected because the
/// generated adapter cannot correctly invoke those adapter shapes or uphold
/// an unsafe function's safety contract. Ordinary generic, const, and
/// explicitly ABI-qualified functions are supported. Const functions run
/// under Criterion and Gungraun but do not participate in runtime allocation
/// or Linux perf tracking.
pub fn benchmark(arguments: TokenStream2, item: TokenStream2) -> Result<TokenStream2> {
    reject_if_too_complex(&arguments)?;
    let arguments = parse2(arguments)?;
    let metabench = metabench_path()?;
    let function = parse2(item)?;
    expand_benchmark(arguments, function, &metabench)
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use quote::quote;
    use syn::ItemFn;

    use super::{BenchmarkArguments, adapter_signature, expand_benchmark, partition_attributes};

    fn compact(tokens: &proc_macro2::TokenStream) -> String {
        tokens.to_string().split_whitespace().collect()
    }

    fn expand(arguments: proc_macro2::TokenStream, function: proc_macro2::TokenStream) -> syn::Result<String> {
        let arguments = syn::parse2::<BenchmarkArguments>(arguments)?;
        let function = syn::parse2::<ItemFn>(function)?;
        expand_benchmark(arguments, function, &quote!(::metabench)).map(|tokens| compact(&tokens))
    }

    #[test]
    fn positional_and_named_forms_expand_equivalently() {
        let positional = expand(
            quote!(IDENTITY, GROUP, "name"),
            quote!(
                fn workload() {}
            ),
        )
        .unwrap();
        let named = expand(
            quote!(identity = IDENTITY, group_name = GROUP, benchmark_name = "name"),
            quote!(
                fn workload() {}
            ),
        )
        .unwrap();

        assert_eq!(positional, named);
    }

    #[test]
    fn conditional_cases_are_forwarded_only_to_the_adapter() {
        let mut function: ItemFn = syn::parse_quote! {
            #[cfg(feature = "enabled")]
            #[cfg_attr(feature = "enabled", cold, bench::one(1), cfg_attr(unix, benches::many(iter = 1..=2)))]
            #[inline(always)]
            fn workload(value: u64) -> u64 { value }
        };
        let attributes = partition_attributes(std::mem::take(&mut function.attrs)).unwrap();
        let callable_attributes = attributes.callable;
        let conditional_attributes = attributes.conditional;
        let case_attributes = attributes.cases;
        let callable = compact(&quote!(#(#callable_attributes)*));
        let conditional = compact(&quote!(#(#conditional_attributes)*));
        let cases = compact(&quote!(#(#case_attributes)*));

        assert!(callable.contains("#[cfg(feature=\"enabled\")]"));
        assert!(callable.contains("#[cfg_attr(feature=\"enabled\",cold)]"));
        assert!(!callable.contains("bench::one"));
        assert!(!callable.contains("inline"));
        assert_eq!(conditional, "#[cfg(feature=\"enabled\")]");
        assert!(cases.contains("cfg_attr(feature=\"enabled\""));
        assert!(cases.contains("bench::one(1)"));
        assert!(cases.contains("cfg_attr(unix,benches::many(iter=1..=2))"));
        assert!(!cases.contains("cold"));
    }

    #[test]
    fn expansion_uses_private_gungraun_reexport_and_does_not_copy_the_body() {
        let output = expand(
            quote!(
                IDENTITY,
                GROUP,
                "name",
                gungraun_config = config(),
                gungraun_setup = setup,
                gungraun_teardown = teardown
            ),
            quote! {
                fn workload(value: u64) -> u64 {
                    const BODY_SENTINEL: u64 = 41;
                    value + BODY_SENTINEL
                }
            },
        )
        .unwrap();

        assert!(output.contains("::metabench::__private::gungraun::library_benchmark"));
        assert!(!output.contains("::metabench::gungraun"));
        assert_eq!(output.matches("BODY_SENTINEL").count(), 2);
        assert!(output.contains("config=config(),setup=setup,teardown=teardown"));
    }

    #[test]
    fn adapter_supports_generic_const_and_abi_functions() {
        let signatures = [
            quote!(
                fn workload<T: Copy>(value: T) -> T
                where
                    T: Send,
                {
                    value
                }
            ),
            quote!(
                extern "C" fn workload(value: u64) -> u64 {
                    value
                }
            ),
            quote!(
                const fn workload(value: u64) -> u64 {
                    value
                }
            ),
        ];

        for function in signatures {
            expand(quote!(IDENTITY, GROUP, "name"), function).unwrap();
        }
    }

    #[test]
    fn unsafe_benchmark_functions_have_an_actionable_error() {
        let unsafe_fn: ItemFn = syn::parse_quote!(
            unsafe fn workload(value: u64) -> u64 {
                value
            }
        );
        let unsafe_error = adapter_signature(&unsafe_fn.sig, &syn::parse_quote!(adapter)).unwrap_err();
        assert!(unsafe_error.to_string().contains("unsafe benchmark functions are unsupported"));
    }

    #[test]
    fn receivers_and_variadics_have_actionable_errors() {
        let receiver: ItemFn = syn::parse_quote!(
            fn workload(&self) {}
        );
        let receiver_error = adapter_signature(&receiver.sig, &syn::parse_quote!(adapter)).unwrap_err();
        assert!(receiver_error.to_string().contains("cannot have a receiver"));

        let variadic: ItemFn = syn::parse_quote!(
            unsafe extern "C" fn workload(first: u64, ...) {}
        );
        let variadic_error = adapter_signature(&variadic.sig, &syn::parse_quote!(adapter)).unwrap_err();
        assert!(variadic_error.to_string().contains("variadic benchmark functions are unsupported"));

        let asynchronous: ItemFn = syn::parse_quote!(
            async fn workload() {}
        );
        let asynchronous_error = adapter_signature(&asynchronous.sig, &syn::parse_quote!(adapter)).unwrap_err();
        assert!(asynchronous_error.to_string().contains("async benchmark functions are unsupported"));

        let const_generic: ItemFn = syn::parse_quote!(
            fn workload<const N: usize>(value: [u8; N]) {}
        );
        let const_generic_error = adapter_signature(&const_generic.sig, &syn::parse_quote!(adapter)).unwrap_err();
        assert!(
            const_generic_error
                .to_string()
                .contains("const-generic benchmark functions are unsupported")
        );
    }

    #[test]
    fn malformed_argument_shapes_are_rejected_deterministically() {
        let cases = [
            (quote!(), "expected identifier"),
            (quote!(IDENTITY), "expected `,`"),
            (quote!(IDENTITY, GROUP), "expected `,`"),
            (quote!(identity = IDENTITY, group_name = GROUP), "missing `benchmark_name`"),
            (
                quote!(identity = IDENTITY, identity = OTHER, group_name = GROUP, benchmark_name = "name"),
                "`identity` may only be specified once",
            ),
            (
                quote!(identity = IDENTITY, group_name = GROUP, benchmark_name = "name", unsupported = true),
                "unsupported benchmark option",
            ),
            (
                quote!(
                    identity = IDENTITY,
                    group_name = GROUP,
                    benchmark_name = "name",
                    gungraun_config = first(),
                    gungraun_config = second()
                ),
                "`gungraun_config` may only be specified once",
            ),
            (
                quote!(IDENTITY, group_name = GROUP, benchmark_name = "name"),
                "positional and named benchmark arguments cannot be mixed",
            ),
            (
                quote!(identity = "not-an-ident", group_name = GROUP, benchmark_name = "name"),
                "`identity` must be a Rust identifier",
            ),
            (
                quote!(identity = IDENTITY, group_name = GROUP, benchmark_name = 1),
                "`benchmark_name` must be a string literal",
            ),
        ];

        for (tokens, expected) in cases {
            let first = syn::parse2::<BenchmarkArguments>(tokens.clone()).err().unwrap().to_string();
            let second = syn::parse2::<BenchmarkArguments>(tokens).err().unwrap().to_string();
            assert_eq!(first, second);
            assert!(first.contains(expected), "{first:?} did not contain {expected:?}");
        }
    }
}
