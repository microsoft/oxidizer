// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-memory macro expansion and diagnostic regression oracles.

#![cfg(feature = "codegen")]

use proc_macro2::TokenStream;
use quote::quote;
use routerama_build::macro_impl::{derive_from_query, derive_to_query, resolver};

#[cfg(test)]
fn diagnostic(tokens: TokenStream) -> String {
    let rendered = tokens.to_string();
    let file = syn::parse2::<syn::File>(tokens).unwrap();
    let messages = file
        .items
        .into_iter()
        .filter_map(|item| {
            let syn::Item::Macro(item) = item else { return None };
            if item.mac.path.segments.last().is_none_or(|segment| segment.ident != "compile_error") {
                return None;
            }
            Some(syn::parse2::<syn::LitStr>(item.mac.tokens).unwrap().value())
        })
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 1, "{messages:?}: {rendered}");
    messages[0].clone()
}

#[test]
fn gamma_survivor_query_attribute_rejection_matrix() {
    check_query_container_attributes();
    check_query_field_attributes();
    check_query_borrowing_attributes();
}

#[cfg(test)]
fn check_query_container_attributes() {
    for (source, expected) in [
        ("enum Q { A }", "query derives support only structs with named fields"),
        ("struct Q(u32);", "query derives support only structs with named fields"),
        ("struct Q;", "query derives support only structs with named fields"),
        (
            "#[query(rename_all=\"PascalCase\")] struct Q { x: u32 }",
            "supported rename rules are camelCase, snake_case, kebab-case, and SCREAMING_SNAKE_CASE",
        ),
        (
            "#[query(rename_all=\"snake_case\",rename_all=\"snake_case\")] struct Q { x: u32 }",
            "duplicate `rename_all` attribute",
        ),
        (
            "#[query(deny_unknown_fields,deny_unknown_fields)] struct Q { x: u32 }",
            "duplicate `deny_unknown_fields` attribute",
        ),
        (
            "#[query(rename=\"x\")] struct Q { x: u32 }",
            "this query attribute is not supported on a container",
        ),
        (
            "#[query(alias=\"x\")] struct Q { x: u32 }",
            "this query attribute is not supported on a container",
        ),
        (
            "#[query(default)] struct Q { x: u32 }",
            "this query attribute is not supported on a container",
        ),
        (
            "#[query(flatten)] struct Q { x: u32 }",
            "this query attribute is not supported on a container",
        ),
        (
            "#[query(skip)] struct Q { x: u32 }",
            "this query attribute is not supported on a container",
        ),
    ] {
        for derive in [derive_from_query, derive_to_query] {
            assert_eq!(diagnostic(derive(source.parse().unwrap())), expected, "{source}");
        }
    }
}

#[cfg(test)]
fn check_query_field_attributes() {
    for (source, expected) in [
        (
            "struct Q { #[query(rename_all=\"snake_case\")] x: u32 }",
            "rename_all and deny_unknown_fields are container attributes",
        ),
        (
            "struct Q { #[query(deny_unknown_fields)] x: u32 }",
            "rename_all and deny_unknown_fields are container attributes",
        ),
        (
            "struct Q { #[query(rename=\"x\",rename=\"x\")] x: u32 }",
            "duplicate `rename` attribute",
        ),
        ("struct Q { #[query(default,default)] x: u32 }", "duplicate `default` attribute"),
        ("struct Q { #[query(flatten,flatten)] x: u32 }", "duplicate `flatten` attribute"),
        ("struct Q { #[query(skip,skip)] x: u32 }", "duplicate `skip` attribute"),
        (
            "struct Q { #[query(repeated)] x: Vec<u32> }",
            "unsupported query attribute `repeated`; Vec<T> fields are repeated automatically",
        ),
        ("struct Q { #[query(typo)] x: u32 }", "unsupported query attribute"),
        (
            "struct Q { #[query(skip,flatten)] x: u32 }",
            "`skip` cannot be combined with other query field attributes",
        ),
        (
            "struct Q { #[query(skip,default)] x: u32 }",
            "`skip` cannot be combined with other query field attributes",
        ),
        (
            "struct Q { #[query(skip,rename=\"x\")] x: u32 }",
            "`skip` cannot be combined with other query field attributes",
        ),
        (
            "struct Q { #[query(skip,alias=\"x\")] x: u32 }",
            "`skip` cannot be combined with other query field attributes",
        ),
        (
            "struct Q { #[query(flatten,default)] x: u32 }",
            "`flatten` cannot be combined with default, rename, or alias",
        ),
        (
            "struct Q { #[query(flatten,rename=\"x\")] x: u32 }",
            "`flatten` cannot be combined with default, rename, or alias",
        ),
        (
            "struct Q { #[query(flatten,alias=\"x\")] x: u32 }",
            "`flatten` cannot be combined with default, rename, or alias",
        ),
        (
            "struct Q { #[query(rename=\"a\")] #[serde(rename=\"b\")] x: u32 }",
            "conflicting query and serde `rename` attributes",
        ),
        (
            "#[query(rename_all=\"snake_case\")] #[serde(rename_all=\"camelCase\")] struct Q { x: u32 }",
            "conflicting query and serde `rename_all` attributes",
        ),
        (
            "struct Q { x: u32, #[query(alias=\"x\")] y: u32 }",
            "duplicate query parameter name `x`",
        ),
        ("struct Q { #[query(alias=\"x\")] x: u32 }", "duplicate query parameter name `x`"),
    ] {
        for derive in [derive_from_query, derive_to_query] {
            assert_eq!(diagnostic(derive(source.parse().unwrap())), expected, "{source}");
        }
    }
}

#[cfg(test)]
fn check_query_borrowing_attributes() {
    for (source, expected) in [
        ("struct Q { x: &str }", "borrowed query fields must declare an explicit lifetime"),
        (
            "struct Q<'a,'b> { x: &'a str, y: Cow<'b,str> }",
            "query data cannot be borrowed through more than one distinct lifetime",
        ),
        (
            "struct Q<'a,'b> { x: Option<&'a str>, y: Vec<&'b str> }",
            "query data cannot be borrowed through more than one distinct lifetime",
        ),
    ] {
        assert_eq!(diagnostic(derive_from_query(source.parse().unwrap())), expected);
    }
    for source in [
        "struct Q<'a,'b> { x: &'a str, y: Cow<'b,str> }",
        "struct Q<'a,'b> { x: Option<&'a str>, y: Vec<&'b str> }",
    ] {
        let encoded = derive_to_query(source.parse().unwrap());
        assert!(!encoded.to_string().contains("compile_error"), "{source}: {encoded}");
        syn::parse2::<syn::File>(encoded).unwrap();
    }
}

#[test]
fn gamma_survivor_query_rename_words_and_serde_merge() {
    for (case, expected) in [
        ("camelCase", "aBCName"),
        ("snake_case", "a_b_c_name"),
        ("kebab-case", "a-b-c-name"),
        ("SCREAMING_SNAKE_CASE", "A_B_C_NAME"),
    ] {
        let source = format!("#[query(rename_all=\"{case}\")] struct Q {{ ABC__Name_: String }}");
        for derive in [derive_from_query, derive_to_query] {
            let output = derive(source.parse().unwrap());
            syn::parse2::<syn::File>(output.clone()).unwrap();
            assert!(output.to_string().contains(&format!("\"{expected}\"")), "{output}");
        }
    }
    let input = quote! {
        #[query(rename_all = "camelCase")]
        #[serde(rename_all = "camelCase", bound(serialize = "ignored"), unknown = "ignored", other)]
        struct Q<'q> {
            #[query(rename = "é +/", alias = "old")]
            #[serde(rename = "é +/", alias = "old", alias = "older")]
            user_name: &'q str,
            #[serde(default)]
            fallback: String,
        }
    };
    let decoded = derive_from_query(input.clone()).to_string();
    assert!(!decoded.contains("compile_error"), "{decoded}");
    assert!(decoded.contains("\"é +/\" | \"old\" | \"older\""), "{decoded}");
    assert!(decoded.contains("fallback : self . fallback . unwrap_or_default"), "{decoded}");
    let encoded = derive_to_query(input).to_string();
    assert!(encoded.contains("\"%C3%A9+%2B%2F\""), "{encoded}");
    assert!(!encoded.contains("\"older\""), "{encoded}");
}

#[test]
fn gamma_survivor_resolver_diagnostic_matrix() {
    for (source, expected) in [
        ("enum Q<'a> { A }", "the capture lifetime must be named `'p`"),
        ("enum Q<T> { A }", "#[resolver] supports only a single `'p` lifetime parameter"),
        (
            "enum Q<const N: usize> { A }",
            "#[resolver] supports only a single `'p` lifetime parameter",
        ),
        ("enum Q<'p: 'static> { A }", "the capture lifetime `'p` cannot have bounds"),
        (
            "enum Q<'p> where Self: Clone { A }",
            "#[resolver] does not support a `where` clause",
        ),
        ("enum r#type { A }", "does not support a raw identifier as the route enum name"),
        ("enum Q { r#type }", "does not support raw route variant identifiers"),
        ("enum Q { #[cfg(any())] A }", "does not support conditionally compiled variants"),
        (
            "enum Q { A { #[cfg_attr(any(),cfg(any()))] x: String } }",
            "does not support conditionally compiled fields",
        ),
        (
            "enum Q<#[cfg(any())] 'p> { A }",
            "does not support conditionally compiled generic parameters",
        ),
        ("enum Q { A {} }", "not empty braces"),
        ("enum Q { A(String) }", "tuple variants are not supported"),
        (
            "enum Q { #[route(GET,\"/{x}\")] A }",
            "fields {} do not match the path captures {x}",
        ),
        (
            "enum Q { #[route(GET,\"/{x}\")] #[route(POST,\"/{y}\")] A { x: String } }",
            "capture different path variables ({x} vs {y})",
        ),
        (
            "enum Q { #[route(\"bad method\",\"/\")] A }",
            "HTTP method strings must be non-empty RFC 9110 `token` values",
        ),
        (
            "enum Q { #[route(\"\",\"/\")] A }",
            "HTTP method strings must be non-empty RFC 9110 `token` values",
        ),
        ("enum Q { #[route(GET,\"/{\")] A }", "invalid path template"),
        (
            "enum Q { #[route(GET,\"/{x}\")] A { x: Option<String> } }",
            "fields cannot be `Option`",
        ),
        ("enum Q { A { x: core::option::Option<String> } }", "fields cannot be `Option`"),
        (
            "enum Q<'p> { #[route(GET,\"/{x}\")] A { x: &'static str } }",
            "must be written as `&'p str`",
        ),
        (
            "enum Q<'p> { #[route(GET,\"/{x}\")] A { x: &'p mut str } }",
            "must be written as `&'p str`",
        ),
        (
            "enum Q<'p> { #[route(GET,\"/{x}\")] A { x: Cow<'p,[u8]> } }",
            "must be written as `Cow<'p, str>`",
        ),
        ("enum Q<'p> { A { x: &'p str } }", "cannot borrow the path"),
        ("enum Q<'p> { A { x: std::borrow::Cow<'p,str> } }", "cannot use `Cow`"),
        ("enum Q<'p> { A { x: Wrapper<'p> } }", "cannot carry the request lifetime `'p`"),
        ("enum Q { AB, A_b }", "both generate `add_a_b`"),
        ("enum Q { builder }", "collides with a generated associated function"),
        (
            "enum Q { #[route(GET,\"/\")] resolver }",
            "collides with a generated associated function",
        ),
    ] {
        let message = diagnostic(resolver(quote! {}, source.parse().unwrap()));
        assert!(message.contains(expected), "{source}: {message}");
    }
    for (attrs, expected) in [
        (quote! { name = r#type }, "the resolver type name cannot be a raw identifier"),
        (quote! { other = X }, "expected `name = ResolverType`"),
        (quote! { name = X, extra }, "unexpected resolver attribute argument"),
        (quote! { name = Q }, "the resolver type name must differ from the route enum name"),
    ] {
        assert_eq!(diagnostic(resolver(attrs, quote! { enum Q { A } })), expected);
    }
}

#[test]
fn gamma_survivor_codegen_generics_and_type_classification() {
    let input = quote! {
        struct Q<'a, T = u32, const N: usize = 3> where T: Trait<Self> {
            raw: &'a str,
            owned: std::string::String,
            cow: alloc::borrow::Cow<'a, str>,
            optional: core::option::Option<T>,
            repeated: alloc::vec::Vec<T>,
            #[query(flatten)]
            child: Nested<T>,
            #[query(skip)]
            skipped: Wrapper<'a, T, N>,
        }
    };
    for (decoding, derive) in [
        (true, derive_from_query as fn(TokenStream) -> TokenStream),
        (false, derive_to_query),
    ] {
        let output = derive(input.clone());
        let rendered = output.to_string();
        assert!(!rendered.contains("compile_error"), "{rendered}");
        syn::parse2::<syn::File>(output).unwrap();
        assert!(rendered.contains("T : Trait"), "{rendered}");
        assert!(rendered.contains("const N : usize"), "{rendered}");
        if decoding {
            for token in [
                "parse_borrowed",
                "parse_owned",
                "parse_cow",
                "Repeated",
                "DecodeFields",
                "Wrapper < 'a , T , N > : :: core :: default :: Default",
                "Trait < Q < 'a , T , N > >",
                "struct __RouteramaQueryState < 'a , T , const N : usize , __RouteramaFlattenDecoder0 >",
                "for __RouteramaQueryState < 'a , T , N , __RouteramaFlattenDecoder0 >",
                "__RouteramaFlattenDecoder0 : :: routerama :: query :: QueryDecoder < 'a , Output = Nested < T > >",
                "Nested < T > : :: routerama :: query :: DecodeFields < 'a >",
            ] {
                assert!(rendered.contains(token), "{token}: {rendered}");
            }
        } else {
            for token in [
                "pair_str",
                "pair_display",
                "EncodeFields",
                "Nested < T > : :: routerama :: query :: EncodeFields",
            ] {
                assert!(rendered.contains(token), "{rendered}");
            }
            assert!(!rendered.contains("Wrapper < 'a , T , N > :"));
        }
    }
    for ty in [
        "custom::String",
        "custom::Cow<'a,str>",
        "custom::Option<u32>",
        "custom::Vec<u32>",
        "&'a mut str",
        "Cow<'a,[u8]>",
        "Option<'a>",
        "Vec<u32,u64>",
        "Option<>",
        "Vec<>",
        "String::Nested",
        "Vec::Nested<u32>",
        "Option::Nested<u32>",
        "Cow<'a>",
    ] {
        let input: TokenStream = format!("struct Q<'a> {{ value: {ty} }}").parse().unwrap();
        assert!(derive_from_query(input.clone()).to_string().contains("parse_value"), "{ty}");
        assert!(derive_to_query(input).to_string().contains("pair_display"), "{ty}");
    }
}

#[test]
fn gamma_survivor_query_syntax_and_helper_identifier_collision_boundaries() {
    for source in [
        "struct Q { #[query(rename)] x: u32 }",
        "struct Q { #[query(rename=42)] x: u32 }",
        "#[query(rename_all)] struct Q { x: u32 }",
        "#[query(rename_all=42)] struct Q { x: u32 }",
        "struct Q { #[query(alias)] x: u32 }",
        "struct Q { #[query(alias=42)] x: u32 }",
        "#[serde(unknown=)] struct Q { x: u32 }",
        "#[serde(rename_all=42)] struct Q { x: u32 }",
        "struct Q { #[serde(rename=42)] x: u32 }",
    ] {
        for derive in [derive_from_query, derive_to_query] {
            assert!(!diagnostic(derive(source.parse().unwrap())).is_empty(), "{source}");
        }
    }
    let input = quote! {
        struct Q<'__routerama_q, '__routerama_q_, __RouteramaQueryState, __RouteramaFlattenDecoder0, __RouteramaQueryWriter> {
            #[query(skip)]
            marker: PhantomData<(&'__routerama_q (), &'__routerama_q_ (), __RouteramaQueryState, __RouteramaFlattenDecoder0, __RouteramaQueryWriter)>,
            #[query(flatten)]
            child: Child,
            __routerama_query_lifetime: u32,
            __routerama_query_lifetime_: u32,
        }
    };
    let decoded = derive_from_query(input.clone());
    syn::parse2::<syn::File>(decoded.clone()).unwrap();
    let decoded = decoded.to_string();
    for token in [
        "'__routerama_q__",
        "struct __RouteramaQueryState_",
        "__RouteramaFlattenDecoder0_",
        "__routerama_query_lifetime__ : :: core :: marker :: PhantomData",
    ] {
        assert!(decoded.contains(token), "{token}: {decoded}");
    }
    let encoded = derive_to_query(input).to_string();
    assert!(encoded.contains("encode_fields < __RouteramaQueryWriter_"), "{encoded}");
    let harmless = derive_from_query(quote! {
        #[serde(deny_unknown_fields, unknown(=))] struct Q { value: u32 }
    })
    .to_string();
    assert!(harmless.contains("const DENY_UNKNOWN_FIELDS : bool = false"), "{harmless}");
    let borrowed_after_scalar = derive_from_query(quote! {
        struct Q<'q> { first: u32, raw: &'q str }
    })
    .to_string();
    assert!(borrowed_after_scalar.contains("DecodeFields < 'q >"), "{borrowed_after_scalar}");
    assert!(!borrowed_after_scalar.contains("'__routerama_q"), "{borrowed_after_scalar}");
    let safe_name = derive_to_query(quote! {
        struct Q { #[query(rename = "A_z-.*09")] value: String }
    })
    .to_string();
    assert!(safe_name.contains("\"A_z-.*09\" , \"A_z-.*09\""), "{safe_name}");
}

#[test]
fn gamma_survivor_resolver_parser_and_shadowed_borrow_lifetimes() {
    for attrs in [quote! { name }, quote! { name : X }, quote! { name = }, quote! { name = X,, }] {
        assert!(!diagnostic(resolver(attrs, quote! { enum Q { A } })).is_empty());
    }
    for source in [
        "enum Q { #[route()] A }",
        "enum Q { #[route(GET)] A }",
        "enum Q { #[route(GET,)] A }",
        "enum Q { #[route(GET,42)] A }",
        "enum Q { #[route(GET,\"/\",extra)] A }",
        "enum Q { #[cfg_attr(any(),cfg(any()))] A }",
    ] {
        assert!(!diagnostic(resolver(quote! {}, source.parse().unwrap())).is_empty(), "{source}");
    }
    let output = resolver(
        quote! {},
        quote! {
            enum Q<'p> { Callback { handler: for<'p, 'q> fn(&'p str, &'q str) } }
        },
    );
    assert!(!output.to_string().contains("compile_error"), "{output}");
    syn::parse2::<syn::File>(output).unwrap();
    let output = resolver(quote! {}, quote! { enum Q { Dynamic { r#type: String }, Unit } }).to_string();
    assert!(output.contains("type : String"), "{output}");
    assert!(output.contains("add_unit"), "{output}");
    for ty in ["&'p [u8]", "String::Nested", "Option::Nested<String>", "Wrapper<'p,str>"] {
        let source = format!("enum Q<'p> {{ #[route(GET,\"/{{x}}\")] A {{ x: {ty} }} }}");
        let output = resolver(quote! {}, source.parse().unwrap()).to_string();
        assert!(!output.contains("compile_error"), "{ty}: {output}");
        assert!(output.contains("coerce_parse"), "{ty}: {output}");
    }
}
