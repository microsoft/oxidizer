// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashSet;

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Error, Expr, ExprLit, Ident, Lit, LitBool, MetaNameValue, Path, Result, Token, braced, bracketed, parse2, token};

use crate::shared::{metabench_path, parse_ident_value, reject_if_too_complex, support_ident};

fn expect_field(input: ParseStream<'_>, expected: &str) -> Result<()> {
    let field = input.parse::<Ident>()?;
    if field == expected {
        Ok(())
    } else {
        Err(Error::new_spanned(field, format!("expected `{expected}`")))
    }
}

fn next_field_is(input: ParseStream<'_>, expected: &str) -> bool {
    input.fork().parse::<Ident>().is_ok_and(|field| field == expected)
}

struct Target {
    criterion: CriterionRegistration,
    native_options: TokenStream2,
    groups: Vec<Group>,
    allocator: Option<Path>,
}

enum CriterionRegistration {
    Default(Path),
    Configured(TokenStream2),
}

struct Group {
    name: Ident,
    benchmarks: Punctuated<Ident, Token![,]>,
    config: Option<Expr>,
    compare_by_id: Option<LitBool>,
    max_parallel: Option<Expr>,
    setup: Option<Expr>,
    teardown: Option<Expr>,
}

impl Group {
    fn simple(benchmark: Ident) -> Self {
        let mut benchmarks = Punctuated::new();
        benchmarks.push(benchmark.clone());
        Self {
            name: benchmark,
            benchmarks,
            config: None,
            compare_by_id: None,
            max_parallel: None,
            setup: None,
            teardown: None,
        }
    }
}

impl Parse for Group {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let name = input.parse()?;
        let content;
        braced!(content in input);
        let fields = Punctuated::<MetaNameValue, Token![,]>::parse_terminated(&content)?;
        let mut benchmarks = None;
        let mut config = None;
        let mut compare_by_id = None;
        let mut max_parallel = None;
        let mut setup = None;
        let mut teardown = None;

        for field in fields {
            if field.path.is_ident("benchmarks") {
                if benchmarks.is_some() {
                    return Err(Error::new_spanned(field, "`benchmarks` may only be specified once"));
                }
                let Expr::Array(array) = field.value else {
                    return Err(Error::new_spanned(field.value, "`benchmarks` must be an array of identity names"));
                };
                let mut identities = Punctuated::new();
                for element in &array.elems {
                    identities.push(parse_ident_value(element, "benchmark identity")?);
                }
                if identities.is_empty() {
                    return Err(Error::new_spanned(array, "a group must contain at least one benchmark"));
                }
                benchmarks = Some(identities);
            } else if field.path.is_ident("gungraun_config") {
                set_once(&mut config, field.value, &field.path)?;
            } else if field.path.is_ident("gungraun_compare_by_id") {
                let Expr::Lit(ExprLit { lit: Lit::Bool(value), .. }) = field.value else {
                    return Err(Error::new_spanned(
                        field.value,
                        "`gungraun_compare_by_id` must be a boolean literal",
                    ));
                };
                set_once(&mut compare_by_id, value, &field.path)?;
            } else if field.path.is_ident("gungraun_max_parallel") {
                set_once(&mut max_parallel, field.value, &field.path)?;
            } else if field.path.is_ident("gungraun_setup") {
                set_once(&mut setup, field.value, &field.path)?;
            } else if field.path.is_ident("gungraun_teardown") {
                set_once(&mut teardown, field.value, &field.path)?;
            } else {
                return Err(Error::new_spanned(field.path, "unsupported group option"));
            }
        }

        Ok(Self {
            name,
            benchmarks: benchmarks.ok_or_else(|| content.error("missing `benchmarks`"))?,
            config,
            compare_by_id,
            max_parallel,
            setup,
            teardown,
        })
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, path: &Path) -> Result<()> {
    if slot.replace(value).is_some() {
        Err(Error::new_spanned(path, "group option may only be specified once"))
    } else {
        Ok(())
    }
}

impl Parse for Target {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        expect_field(input, "criterion")?;
        input.parse::<Token![=]>()?;
        let criterion = if input.peek(token::Brace) {
            let configured;
            braced!(configured in input);
            CriterionRegistration::Configured(configured.parse()?)
        } else {
            CriterionRegistration::Default(input.parse()?)
        };
        input.parse::<Token![,]>()?;

        let native_options = if next_field_is(input, "gungraun") {
            expect_field(input, "gungraun")?;
            input.parse::<Token![=]>()?;
            let native_options;
            braced!(native_options in input);
            input.parse::<Token![,]>()?;
            native_options.parse()?
        } else {
            TokenStream2::new()
        };

        let groups: Vec<Group> = if next_field_is(input, "benchmarks") {
            expect_field(input, "benchmarks")?;
            input.parse::<Token![=]>()?;
            let benchmarks;
            bracketed!(benchmarks in input);
            let benchmarks = Punctuated::<Ident, Token![,]>::parse_terminated(&benchmarks)?;
            if benchmarks.is_empty() {
                return Err(input.error("a target must contain at least one benchmark"));
            }
            benchmarks.into_iter().map(Group::simple).collect()
        } else {
            expect_field(input, "groups")?;
            input.parse::<Token![=]>()?;
            let groups;
            braced!(groups in input);
            let groups = Punctuated::<Group, Token![,]>::parse_terminated(&groups)?;
            if groups.is_empty() {
                return Err(input.error("a target must contain at least one group"));
            }
            groups.into_iter().collect()
        };
        let mut seen_groups = HashSet::new();
        let mut seen_benchmarks = HashSet::new();
        for group in &groups {
            if !seen_groups.insert(group.name.to_string()) {
                return Err(Error::new_spanned(&group.name, "a group may only be declared once"));
            }
            for benchmark in &group.benchmarks {
                if !seen_benchmarks.insert(benchmark.to_string()) {
                    return Err(Error::new_spanned(benchmark, "a benchmark identity may only belong to one group"));
                }
            }
        }

        let allocator = if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                None
            } else {
                expect_field(input, "allocator")?;
                input.parse::<Token![=]>()?;
                let allocator = Some(input.parse()?);
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                }
                allocator
            }
        } else {
            None
        };
        if !input.is_empty() {
            return Err(input.error("unexpected target option"));
        }

        Ok(Self {
            criterion,
            native_options,
            groups,
            allocator,
        })
    }
}

fn expand_target(target: Target, metabench: &TokenStream2) -> TokenStream2 {
    let Target {
        criterion,
        native_options,
        groups,
        allocator,
    } = target;
    let criterion = match criterion {
        CriterionRegistration::Default(benchmarks) => quote!(#benchmarks),
        CriterionRegistration::Configured(configuration) => quote!({ #configuration }),
    };
    let group_declarations = groups.iter().map(|group| {
        let name = &group.name;
        let logical_group = name.to_string();
        let group_name = support_ident("group", &logical_group);
        let group_module = support_ident("group_module", &logical_group);
        let adapter_names = group
            .benchmarks
            .iter()
            .map(|identity| support_ident("benchmark", &identity.to_string()))
            .collect::<Vec<_>>();
        let adapter_imports = group.benchmarks.iter().zip(&adapter_names).map(|(identity, adapter_name)| {
            let adapter_module = support_ident("native", &identity.to_string());
            quote!(use super::#adapter_module::#adapter_name;)
        });
        let config = group.config.as_ref().map(|value| quote!(config = #value;));
        let compare_by_id = group.compare_by_id.as_ref().map(|value| quote!(compare_by_id = #value;));
        let max_parallel = group.max_parallel.as_ref().map(|value| quote!(max_parallel = #value;));
        let setup = group.setup.as_ref().map(|value| quote!(setup = #value;));
        let teardown = group.teardown.as_ref().map(|value| quote!(teardown = #value;));

        quote! {
            mod #group_module {
                use super::*;
                use #metabench::__private::gungraun;
                #(#adapter_imports)*

                #metabench::__private::gungraun::library_benchmark_group!(
                    name = #group_name;
                    #config
                    #compare_by_id
                    #max_parallel
                    #setup
                    #teardown
                    benchmarks = #(#adapter_names),*
                );
            }

            use #group_module::#group_name;
        }
    });
    let group_names = groups.iter().map(|group| support_ident("group", &group.name.to_string()));
    let identities = groups.iter().flat_map(|group| {
        let native_group = support_ident("group", &group.name.to_string());
        group
            .benchmarks
            .iter()
            .map(move |identity| quote!(#identity.__in_gungraun_group(stringify!(#native_group))))
    });
    let allocator = allocator.map(|allocator| quote!(, allocator = #allocator));
    let target_key = groups
        .iter()
        .flat_map(|group| std::iter::once(&group.name).chain(group.benchmarks.iter()))
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(":");
    let identities_name = support_ident("identities", &target_key);
    let target_module = support_ident("target", &target_key);

    quote! {
        #(#group_declarations)*

        const #identities_name: &[#metabench::BenchmarkIdentity] = &[
            #(#identities),*
        ];

        mod #target_module {
            use super::*;

            #metabench::__metabench_main!(
                criterion = #criterion,
                gungraun = {
                    #native_options
                    library_benchmark_groups = #(#group_names),*
                },
                identities = #identities_name
                #allocator
            );

            pub(super) fn run() {
                main();
            }
        }

        fn main() {
            #target_module::run();
        }
    }
}

/// Generates private native Gungraun groups and the target entry point.
pub fn main(input: TokenStream2) -> Result<TokenStream2> {
    reject_if_too_complex(&input)?;
    let target = parse2(input)?;
    let metabench = metabench_path()?;
    Ok(expand_target(target, &metabench))
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use quote::quote;

    use super::{Target, expand_target};

    fn compact(tokens: &proc_macro2::TokenStream) -> String {
        tokens.to_string().split_whitespace().collect()
    }

    fn expand(input: proc_macro2::TokenStream) -> syn::Result<String> {
        syn::parse2::<Target>(input).map(|target| compact(&expand_target(target, &quote!(::metabench))))
    }

    #[test]
    fn simple_and_configured_targets_expand() {
        let simple = expand(quote!(
            criterion = criterion_benches,
            benchmarks = [ONE, TWO],
            allocator = std::alloc::System
        ))
        .unwrap();
        assert!(simple.contains("criterion=criterion_benches"));
        assert!(simple.contains("allocator=std::alloc::System"));
        assert!(simple.contains("ONE.__in_gungraun_group"));
        assert!(simple.contains("TWO.__in_gungraun_group"));

        let configured = expand(quote! {
            criterion = {
                factory = configured_criterion,
                benchmarks = criterion_benches,
                unit = "cycles",
            },
            gungraun = {
                config = suite_config();
                setup = suite_setup();
                teardown = suite_teardown();
            },
            groups = {
                GROUP {
                    benchmarks = [ONE, TWO],
                    gungraun_config = group_config(),
                    gungraun_compare_by_id = true,
                    gungraun_max_parallel = 2,
                    gungraun_setup = group_setup(),
                    gungraun_teardown = group_teardown(),
                },
            },
        })
        .unwrap();
        assert!(configured.contains("factory=configured_criterion"));
        assert!(configured.contains("unit=\"cycles\""));
        assert!(configured.contains("compare_by_id=true"));
        assert!(configured.contains("max_parallel=2"));
    }

    #[test]
    fn all_generated_gungraun_paths_use_the_private_reexport() {
        let output = expand(quote!(criterion = criterion_benches, benchmarks = [ONE])).unwrap();

        assert!(output.contains("use::metabench::__private::gungraun"));
        assert!(output.contains("::metabench::__private::gungraun::library_benchmark_group!"));
        assert!(!output.contains("::metabench::gungraun"));
    }

    #[test]
    fn malformed_target_shapes_are_rejected_deterministically() {
        let cases = [
            (quote!(), "expected identifier"),
            (quote!(benchmarks = [ONE], criterion = benches), "expected `criterion`"),
            (quote!(criterion = benches), "expected `,`"),
            (quote!(criterion = benches, benchmarks = []), "at least one benchmark"),
            (quote!(criterion = benches, groups = {}), "at least one group"),
            (
                quote!(criterion = benches, groups = { GROUP { benchmarks = [] } }),
                "at least one benchmark",
            ),
            (
                quote!(criterion = benches, groups = { GROUP { gungraun_config = config() } }),
                "missing `benchmarks`",
            ),
            (
                quote!(criterion = benches, groups = { GROUP { benchmarks = [ONE], unsupported = true } }),
                "unsupported group option",
            ),
            (
                quote!(
                    criterion = benches,
                    groups = {
                        GROUP { benchmarks = [ONE] },
                        GROUP { benchmarks = [TWO] },
                    }
                ),
                "group may only be declared once",
            ),
            (
                quote!(
                    criterion = benches,
                    groups = {
                        FIRST { benchmarks = [ONE] },
                        SECOND { benchmarks = [ONE] },
                    }
                ),
                "benchmark identity may only belong to one group",
            ),
            (
                quote!(
                    criterion = benches,
                    groups = { GROUP { benchmarks = [ONE], gungraun_compare_by_id = expression() } }
                ),
                "must be a boolean literal",
            ),
            (
                quote!(
                    criterion = benches,
                    groups = { GROUP { benchmarks = [ONE], gungraun_setup = first, gungraun_setup = second } }
                ),
                "group option may only be specified once",
            ),
            (
                quote!(criterion = benches, benchmarks = [ONE], unexpected = true),
                "expected `allocator`",
            ),
            (
                quote!(
                    criterion = benches,
                    benchmarks = [ONE],
                    allocator = std::alloc::System,
                    trailing = true
                ),
                "unexpected target option",
            ),
        ];

        for (tokens, expected) in cases {
            let first = syn::parse2::<Target>(tokens.clone()).err().unwrap().to_string();
            let second = syn::parse2::<Target>(tokens).err().unwrap().to_string();
            assert_eq!(first, second);
            assert!(first.contains(expected), "{first:?} did not contain {expected:?}");
        }
    }

    #[test]
    fn nested_token_shapes_never_panic_and_preserve_acceptance() {
        let expressions = [
            quote!(factory()),
            quote!({ factory() }),
            quote!((factory(), other()).0),
            quote!(factory::<Vec<(u8, u16)>>()),
            quote!(if cfg!(unix) { first() } else { second() }),
        ];

        for expression in expressions {
            let input = quote! {
                criterion = benches,
                groups = {
                    GROUP {
                        benchmarks = [ONE],
                        gungraun_config = #expression,
                        gungraun_max_parallel = #expression,
                        gungraun_setup = #expression,
                        gungraun_teardown = #expression,
                    },
                },
            };
            let first = syn::parse2::<Target>(input.clone()).map(|target| compact(&expand_target(target, &quote!(::metabench))));
            let second = syn::parse2::<Target>(input).map(|target| compact(&expand_target(target, &quote!(::metabench))));
            assert_eq!(first.unwrap(), second.unwrap());
        }
    }
}
