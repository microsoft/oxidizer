// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated enum structure and diagnostic oracles.

#![cfg(feature = "codegen")]

use http_path_template::{Grammar, PathTemplate};
use quote::quote;
use routerama_build::macro_impl::{derive_from_query, resolver};
use routerama_build::{Generator, Route};

fn route(name: &str, path: &str) -> Route {
    Route::new(
        name,
        "GET",
        PathTemplate::parse(path, Grammar::default()).expect("the fixture path is a valid route"),
    )
}

#[test]
fn gamma_nov4_affix_order_accepts_an_empty_public_trie_edge_without_underflow() {
    use routerama_build::trie::{Node, affix_edges_in_match_order};
    let mut node = Node::default();
    for (prefix, suffix) in [("", ""), ("a", ""), ("", "bc"), ("ab", "")] {
        node.affix.insert((prefix.into(), suffix.into()), Node::default());
    }
    let ordered = affix_edges_in_match_order(&node);
    assert_eq!(
        ordered
            .iter()
            .map(|((prefix, suffix), _)| (prefix.as_str(), suffix.as_str()))
            .collect::<Vec<_>>(),
        [("", "bc"), ("ab", ""), ("a", ""), ("", "")]
    );
}

#[test]
fn gamma_nov4_invalid_first_resolver_attribute_token_becomes_a_diagnostic_instead_of_panicking() {
    for attributes in [quote!(= Name), quote!(17 = Name), quote!(,)] {
        let output = resolver(
            attributes,
            quote!(
                enum Routes {
                    #[route(GET, "/")]
                    Home,
                }
            ),
        );
        syn::parse2::<syn::File>(output.clone()).unwrap();
        assert!(output.to_string().contains("compile_error"), "{output}");
        assert!(output.to_string().contains("expected identifier"), "{output}");
    }
}

#[test]
fn gamma_nov4_dynamic_raw_identifiers_retain_exact_owned_and_parsed_capture_keys() {
    let output = resolver(
        quote!(),
        quote!(
            enum Routes {
                Dynamic { r#type: String, r#match: u32 },
            }
        ),
    );
    syn::parse2::<syn::File>(output.clone()).unwrap();
    let compact = output.to_string().split_whitespace().collect::<String>();
    for expected in [
        quote!(r#type: ::routerama::__rt::owned(__caps, 0usize, "type")?),
        quote!(r#match: ::routerama::__rt::parse(__caps, 1usize, "match")?),
    ] {
        let expected = expected.to_string().split_whitespace().collect::<String>();
        assert!(compact.contains(&expected), "{expected}: {compact}");
    }
}

#[test]
fn gamma_nov4_resolver_topologies_emit_only_the_required_static_dynamic_and_fallback_functions() {
    for (item, expected_static, expected_dynamic, expected_raw, expected_fallback) in [
        (
            quote!(
                enum Empty {}
            ),
            true,
            false,
            false,
            false,
        ),
        (
            quote!(
                enum DynamicOnly {
                    Entry,
                }
            ),
            false,
            true,
            false,
            false,
        ),
        (
            quote!(
                enum StaticOnly {
                    #[route(GET, "/ready")]
                    Ready,
                }
            ),
            true,
            false,
            true,
            false,
        ),
        (
            quote!(
                enum Mixed {
                    #[route(GET, "/ready:go")]
                    Ready,
                    Dynamic,
                }
            ),
            true,
            true,
            true,
            true,
        ),
    ] {
        let output = resolver(quote!(), item);
        let file = syn::parse2::<syn::File>(output.clone()).unwrap();
        let compact = output.to_string().split_whitespace().collect::<String>();
        assert!(!compact.contains("compile_error"), "{compact}");
        assert_eq!(compact.contains("fn__static_resolve"), expected_static, "{compact}");
        assert_eq!(compact.contains("fn__dynamic_resolve"), expected_dynamic, "{compact}");
        assert_eq!(compact.contains("fn__convert_static"), expected_raw, "{compact}");
        assert_eq!(
            compact.contains("match__static_resolve(__method,__path)"),
            expected_fallback,
            "{compact}"
        );
        if expected_raw {
            assert!(
                compact.contains("\"ready\""),
                "the actual ready route must reach static generation: {compact}"
            );
        }
        let implementation = file
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Impl(item)
                    if item
                        .trait_
                        .as_ref()
                        .is_some_and(|(path, _)| path.segments.last().unwrap().ident == "Resolver") =>
                {
                    Some(item)
                }
                _ => None,
            })
            .unwrap();
        let tail = implementation
            .items
            .iter()
            .find_map(|item| match item {
                syn::ImplItem::Fn(item) if item.sig.ident == "resolve" => item.block.stmts.last(),
                _ => None,
            })
            .unwrap();
        let tail = quote!(#tail).to_string().split_whitespace().collect::<String>();
        if !expected_fallback {
            assert_eq!(
                tail,
                if expected_dynamic {
                    "__dynamic_resolve(__state,__method,__path)"
                } else {
                    "__static_resolve(__method,__path)"
                }
            );
        }
    }
}

#[test]
fn gamma_nov4_generated_name_and_capture_arms_preserve_unit_and_named_patterns() {
    let mut generator = Generator::new("Routes", true);
    generator
        .full_api(true)
        .add_all([route("Home", "/"), route("Pair", "/{book.id}/{name}")]);
    let output = generator.generate().to_string().split_whitespace().collect::<String>();
    for arm in [
        quote!(Routes::Home => "Home",),
        quote!(Routes::Pair { .. } => "Pair",),
        quote!(Routes::Home => ::core::option::Option::None,),
        quote!(Routes::Pair { book_id: __cap_book_id, name: __cap_name } => match __key),
    ] {
        let expected = arm.to_string().split_whitespace().collect::<String>();
        assert!(output.contains(&expected), "missing {expected} in {output}");
    }
}

#[test]
fn gamma_nov4_rewritten_query_where_groups_keep_their_original_source_span() {
    use proc_macro2::{Delimiter, TokenTree};

    let input = "struct Query<T> where Self: Trait<(u8, Self)> { value: T }"
        .parse()
        .expect("the generic query fixture has valid Rust token syntax");
    let output = derive_from_query(input);
    syn::parse2::<syn::File>(output.clone()).expect("the rewritten query emits valid Rust syntax");
    let mut pending = vec![output];
    let mut rewritten = 0;
    while let Some(tokens) = pending.pop() {
        for token in tokens {
            if let TokenTree::Group(group) = token {
                let contents = group.stream();
                if group.delimiter() == Delimiter::Parenthesis
                    && contents.to_string().split_whitespace().collect::<String>() == "u8,Query<T>"
                {
                    rewritten += 1;
                    assert_eq!(group.span().source_text().as_deref(), Some("(u8, Self)"));
                }
                pending.push(contents);
            }
        }
    }
    assert!(rewritten > 0, "the generated visitor must rewrite Self in its where clause");
}

#[test]
fn gamma_nov4_field_sanitization_replaces_nul_without_losing_neighboring_underscores() {
    assert_eq!(routerama_build::route_field_name("a_\0_b"), "_f_a___b");
    assert_eq!(routerama_build::route_field_name("a_x_b"), "a_x_b");
}

#[test]
fn gamma_nov4_invalid_variant_nul_is_sanitized_without_panicking() {
    let mut generator = Generator::new("Routes", true);
    generator.add_all([route("Bad\0Name", "/bad")]);
    let output = generator.generate();
    assert!(output.to_string().contains("compile_error"), "{output}");
    let file = syn::parse2::<syn::File>(output).unwrap();
    let variant = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Enum(item) => item.variants.first(),
            _ => None,
        })
        .unwrap();
    assert_eq!(variant.ident, "_R_Bad_Name");
}

#[test]
fn gamma_nov4_conflicting_leaf_fallback_keeps_the_first_declared_return() {
    let mut generator = Generator::new("Routes", true);
    generator
        .full_api(false)
        .add_all([route("First", "/same"), route("Second", "/same")]);
    let output = generator.generate().to_string();
    assert!(output.contains("compile_error"), "{output}");
    assert!(
        output.contains(&quote!(return ::core::option::Option::Some(Routes::First);).to_string()),
        "{output}"
    );
    assert!(
        !output.contains(&quote!(return ::core::option::Option::Some(Routes::Second);).to_string()),
        "{output}"
    );
}

#[test]
fn gamma_nov4_rest_leaves_keep_both_plain_and_custom_verb_dispatch_arms() {
    let mut generator = Generator::new("Routes", true);
    generator
        .full_api(false)
        .add_all([route("Plain", "/{tail=**}"), route("Verb", "/{tail=**}:go")]);
    let output = generator.generate().to_string();
    assert!(!output.contains("compile_error"), "{output}");
    assert!(output.contains("match __verb"), "{output}");
    assert!(
        output.contains(&quote!(::core::option::Option::Some("go") =>).to_string()),
        "{output}"
    );
    assert!(output.contains(&quote!(::core::option::Option::None =>).to_string()), "{output}");
    assert!(output.contains("Routes :: Plain { tail :"), "{output}");
    assert!(output.contains("Routes :: Verb { tail :"), "{output}");
}

#[test]
fn gamma_round2_capture_field_paths_split_affix_dots_and_sanitize_invalid_identifiers() {
    let template = PathTemplate::parse("/pre-{outer.inner}.txt/{other.id}", Grammar::default().with_segment_affixes()).unwrap();
    assert_eq!(
        routerama_build::trie::capture_field_names(template.segments()),
        [
            vec!["outer".to_owned(), "inner".to_owned()],
            vec!["other".to_owned(), "id".to_owned()]
        ]
    );
    assert_eq!(routerama_build::route_field_name("9_bad?"), "_f_9_bad_");
    assert_eq!(routerama_build::route_field_name("outer.inner"), "outer_inner");
}

#[test]
fn gamma_round2_conflict_diagnostics_keep_literal_then_affix_lexical_order() {
    let mut routes = Vec::new();
    for (index, path) in ["/z", "/a", "/z-{id}", "/a-{id}"].into_iter().enumerate() {
        for duplicate in 0..2 {
            routes.push(Route::new(
                format!("Route{index}_{duplicate}"),
                "GET",
                PathTemplate::parse(path, Grammar::default().with_segment_affixes()).unwrap(),
            ));
        }
    }
    let trie = routerama_build::trie::build_trie(&routes);
    let conflicts = routerama_build::trie::conflicts(&trie.root);
    assert_eq!(
        conflicts
            .iter()
            .map(|message| message.split('`').nth(1).unwrap())
            .collect::<Vec<_>>(),
        ["GET /a", "GET /z", "GET /a-{}", "GET /z-{}"]
    );
}

#[test]
fn gamma_round2_resolver_aliases_accept_differently_ordered_capture_declarations() {
    let output = resolver(
        quote!(),
        quote!(
            enum Routes<'p> {
                #[route(GET, "/first/{a}/{b}")]
                #[route(GET, "/second/{b}/{a}")]
                Both { a: &'p str, b: &'p str },
            }
        ),
    );
    assert!(!output.to_string().contains("compile_error"), "{output}");
    syn::parse2::<syn::File>(output).unwrap();
}

#[test]
fn gamma_round2_unit_resolver_arms_stay_unit_shaped_without_spurious_static_or_verb_code() {
    let output = resolver(
        quote!(),
        quote!(
            enum Shapes {
                #[route(GET, "/")]
                Ready,
                Dynamic,
            }
        ),
    )
    .to_string()
    .split_whitespace()
    .collect::<String>();
    for expected in [
        "__ShapesRaw::Ready=>Shapes::Ready,",
        "Result::Ok(Shapes::Dynamic)",
        "Shapes::Dynamic=>Shapes::Dynamic,",
        "force_verb_split(&mut__resolver,false)",
    ] {
        assert!(output.contains(expected), "{expected}: {output}");
    }
    let dynamic = resolver(
        quote!(),
        quote!(
            enum OnlyDynamic {
                Entry,
            }
        ),
    );
    assert!(!dynamic.to_string().contains("__OnlyDynamicRaw"), "{dynamic}");
}

#[test]
fn gamma_round2_query_claim_arms_exclude_skipped_and_flattened_fields_and_keep_aliases() {
    let output = derive_from_query(quote!(
        struct Query {
            #[query(alias = "other")]
            value: u32,
            #[query(skip)]
            skipped: u32,
            #[query(flatten)]
            nested: Inner,
        }
    ))
    .to_string()
    .split_whitespace()
    .collect::<String>();
    assert!(output.contains("\"value\"|\"other\"=>true"), "{output}");
    assert!(!output.contains("\"skipped\"=>true"), "{output}");
    assert!(!output.contains("\"nested\"=>true"), "{output}");
    for attribute in [quote!(#[query(skip)]), quote!(#[query(flatten)])] {
        let output = derive_from_query(quote!(
            struct Query {
                #[query(rename = "ignored")]
                regular: u32,
                #attribute
                ignored: Inner,
            }
        ));
        assert!(!output.to_string().contains("compile_error"), "{output}");
    }
}

#[test]
fn gamma_round2_alias_in_middle_does_not_discard_later_variants() {
    for count in [4, 1024] {
        let mut generator = Generator::new("Routes", true);
        generator.add_all([route("First", "/first"), route("First", "/alias"), route("AfterAlias", "/after")]);
        generator.add_all((3..count).map(|index| route(&format!("Route{index}"), &format!("/r{index}"))));
        let file = syn::parse2::<syn::File>(generator.generate()).unwrap();
        let variants = file
            .items
            .iter()
            .find_map(|item| {
                if let syn::Item::Enum(item) = item {
                    Some(&item.variants)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(variants.len(), count - 1);
        assert_eq!(variants[0].ident, "First");
        assert_eq!(variants[1].ident, "AfterAlias");
        assert_eq!(variants.last().unwrap().ident, format!("Route{}", count - 1));
        assert!(variants.iter().all(|variant| matches!(variant.fields, syn::Fields::Unit)));
    }
}

#[test]
fn gamma_round2_generated_enum_distinguishes_unit_and_named_captures() {
    let mut generator = Generator::new("Routes", true);
    generator
        .full_api(true)
        .add_all([route("Home", "/"), route("Pair", "/{book.id}/{name}")]);
    let output = generator.generate();
    let file = syn::parse2::<syn::File>(output.clone()).unwrap();
    let variants = file
        .items
        .iter()
        .find_map(|item| {
            if let syn::Item::Enum(item) = item {
                Some(&item.variants)
            } else {
                None
            }
        })
        .unwrap();
    assert!(matches!(variants[0].fields, syn::Fields::Unit));
    let syn::Fields::Named(fields) = &variants[1].fields else {
        panic!("capture variant must have named fields")
    };
    assert_eq!(
        fields
            .named
            .iter()
            .map(|field| field.ident.as_ref().unwrap().to_string())
            .collect::<Vec<_>>(),
        ["book_id", "name"]
    );
    assert!(output.to_string().contains("RouteMatch"));
}

#[test]
fn gamma_round2_macro_parser_missing_values_return_compile_errors() {
    for attributes in [quote!(name), quote!(name =), quote!(name = Resolver unexpected)] {
        let generated = resolver(
            attributes,
            quote!(
                enum Routes {
                    #[route(GET, "/")]
                    Home,
                }
            ),
        );
        assert!(generated.to_string().contains("compile_error"), "{generated}");
    }
    for field in [quote!(#[query(rename)] value: u32), quote!(#[query(default =)] value: u32)] {
        let generated = derive_from_query(quote!(struct Query { #field }));
        assert!(generated.to_string().contains("compile_error"), "{generated}");
    }
}

#[test]
fn gamma_round2_resolver_expansion_keeps_schema_and_borrowed_lifetimes_and_minimal_raw_api() {
    let owned = resolver(
        quote!(),
        quote!(
            enum Round2Owned {
                #[route(GET, "/ready")]
                Ready,
            }
        ),
    );
    let owned = owned.to_string().split_whitespace().collect::<String>();
    assert!(!owned.contains("compile_error"), "{owned}");
    assert!(!owned.contains("Round2Owned<"), "{owned}");
    assert!(!owned.contains("__Round2OwnedRaw<"), "{owned}");
    assert!(!owned.contains("RouteMatch"), "{owned}");
    assert!(!owned.contains("pubenum__Round2OwnedRaw"), "{owned}");
    assert!(owned.contains("__Round2OwnedRaw::Ready"), "{owned}");
    let borrowed = resolver(
        quote!(),
        quote!(
            enum Round2Borrowed<'p> {
                #[route(GET, "/{value}:lookup")]
                Borrowed {
                    value: &'p str,
                },
                #[route(GET, "/ready")]
                Ready,
                Dynamic {
                    value: String,
                },
            }
        ),
    );
    let borrowed = borrowed.to_string().split_whitespace().collect::<String>();
    assert!(!borrowed.contains("compile_error"), "{borrowed}");
    assert!(borrowed.contains("Result<Round2Borrowed<'static>,"), "{borrowed}");
    assert!(borrowed.contains("Result<Round2Borrowed<'p>,"), "{borrowed}");
    assert!(borrowed.contains("__raw:__Round2BorrowedRaw<'p>"), "{borrowed}");
    assert!(borrowed.contains("force_verb_split(&mut__resolver,true)"), "{borrowed}");
}

#[test]
fn gamma_round2_trie_payload_indices_and_dotted_affix_capture_keys() {
    use routerama_build::trie::build_trie;
    let routes = [
        route("First", "/first"),
        route("Second", "/second/{nested.id}"),
        Route::new(
            "Third",
            "GET",
            PathTemplate::parse("/pre-{nested.other}.txt", Grammar::default().with_segment_affixes()).unwrap(),
        ),
    ];
    let trie = build_trie(&routes);
    assert_eq!(trie.root.literals["first"].exact[0].route_index, 0);
    let second = &trie.root.literals["second"].single.as_ref().unwrap().exact[0];
    assert_eq!(second.route_index, 1);
    assert_eq!(second.vars[0].key(), "nested.id");
    let third = &trie.root.affix[&("pre-".into(), ".txt".into())].exact[0];
    assert_eq!(third.route_index, 2);
    assert_eq!(third.vars[0].key(), "nested.other");
    assert_eq!(third.vars[0].field(), "nested_other");
}

#[test]
fn gamma_round2_query_decoder_state_contains_every_field_kind_and_initializer() {
    let tokens = derive_from_query(quote! {
        struct Query<T> where T: Into<Self> {
            required: u32,
            optional: Option<u32>,
            repeated: Vec<u32>,
            #[query(flatten)]
            nested: T,
            #[query(skip)]
            skipped: u32,
        }
    });
    syn::parse2::<syn::File>(tokens.clone()).unwrap();
    let output = tokens.to_string();
    assert!(!output.contains("compile_error"), "{output}");
    for expected in [
        quote!(required: ::core::option::Option<u32>),
        quote!(optional: ::core::option::Option<u32>),
        quote!(repeated: ::routerama::query::Repeated<u32>),
        quote!(required: ::core::option::Option::None),
        quote!(optional: ::core::option::Option::None),
        quote!(repeated: ::routerama::query::Repeated::new()),
        quote!(nested: <T as ::routerama::query::DecodeFields),
    ] {
        assert!(output.contains(&expected.to_string()), "{expected}: {output}");
    }
    assert!(output.contains("nested : __RouteramaFlattenDecoder0"), "{output}");
    let compact: String = output.split_whitespace().collect();
    assert_eq!(compact.matches("T:Into<Query<T>>").count(), 2, "{output}");
    assert!(compact.contains("impl<'__routerama_q,T,__RouteramaFlattenDecoder0>"), "{output}");
}
