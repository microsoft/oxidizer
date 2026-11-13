// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process tests for the derive entrypoint's diagnostics.

use multitude_macros_impl::derive_deserialize_in;
use quote::quote;
use syn::parse_quote;

#[test]
fn gamma_survivor_derive_reports_exact_diagnostics() {
    let cases = [
        (
            "struct Input<'de>(&'de str);",
            "`DeserializeIn` reserves the deserializer lifetime name `'de`",
        ),
        ("union Input { field: u8 }", "#[derive(DeserializeIn)] does not support unions"),
        (
            "#[serde(default)] struct Input(u8);",
            "serde container `default` is supported only for named structs",
        ),
        (
            "#[serde(default)] struct Input;",
            "serde container `default` is supported only for named structs",
        ),
        (
            "#[serde(default)] enum Input { Unit }",
            "serde container `default` is supported only for named structs",
        ),
        (
            "#[serde(transparent)] enum Input { Unit }",
            "serde `transparent` is supported only for structs",
        ),
        (
            "#[serde(transparent)] struct Input;",
            "serde `transparent` requires exactly one non-skipped field",
        ),
        (
            "#[serde(transparent)] struct Input(u8, u16);",
            "serde `transparent` requires exactly one non-skipped field",
        ),
        (
            "#[serde(transparent)] struct Input(#[serde(skip)] u8);",
            "serde `transparent` requires exactly one non-skipped field",
        ),
        (
            "#[serde(transparent, default)] struct Input { value: u8 }",
            "serde container `default` cannot be combined with `transparent`",
        ),
        (
            "enum Input { #[serde(default)] Unit }",
            "`default` and `via_serde` are field-only multitude attributes",
        ),
        (
            "enum Input { #[multitude(via_serde)] Unit }",
            "`default` and `via_serde` are field-only multitude attributes",
        ),
        (
            "struct Input { #[serde(alias = \"second\")] first: u8, second: u8 }",
            "duplicate deserialization field name or alias `second`",
        ),
        (
            "enum Input { #[serde(alias = \"Second\")] First, Second }",
            "duplicate deserialization variant name or alias `Second`",
        ),
        (
            "struct Input { #[serde(alias = \"first\")] first: u8 }",
            "duplicate deserialization field name or alias `first`",
        ),
        (
            "enum Input { #[serde(alias = \"First\")] First }",
            "duplicate deserialization variant name or alias `First`",
        ),
    ];
    for (source, message) in cases {
        let output = derive_deserialize_in(source.parse().unwrap(), &parse_quote!(::multitude::de));
        assert_eq!(
            output.to_string(),
            quote! { ::core::compile_error! { #message } }.to_string(),
            "{source}"
        );
    }
    for name in [
        "__D",
        "__M",
        "__S",
        "__E",
        "__FieldVisitor",
        "__VariantVisitor",
        "__Multitude",
        "__MultitudeUser",
    ] {
        for declaration in [name.to_owned(), format!("const {name}: usize")] {
            let source = format!("struct Input<{declaration}>;");
            let message = format!("`DeserializeIn` reserves the internal generic parameter name `{name}`");
            let output = derive_deserialize_in(source.parse().unwrap(), &parse_quote!(::multitude::de));
            assert_eq!(output.to_string(), quote! { ::core::compile_error! { #message } }.to_string());
        }
    }
}

#[test]
fn gamma_survivor_derive_propagates_nested_parser_errors() {
    for source in [
        "struct Input { #[serde(default, default)] value: u8 }",
        "enum Input { #[serde(default, default)] Unit }",
        "enum Input { Named { #[serde(default, default)] value: u8 } }",
        "enum Input { #[serde(deserialize_with = \"decode\")] New(#[serde(default, default)] u8) }",
        "#[serde(transparent)] struct Input { #[serde(default, default)] value: u8 }",
    ] {
        let output = derive_deserialize_in(source.parse().unwrap(), &parse_quote!(::multitude::de));
        assert_eq!(
            output.to_string(),
            quote! { ::core::compile_error! { "duplicate default" } }.to_string(),
        );
    }

    let output = derive_deserialize_in(quote! { not a derive input }, &parse_quote!(::multitude::de));
    assert!(output.to_string().contains("compile_error"));
    let output = derive_deserialize_in(
        quote! { #[multitude(crate = "::renamed")] struct Input { value: u8 } },
        &parse_quote!(::wrong::de),
    );
    syn::parse2::<syn::File>(output.clone()).unwrap();
    let output = output.to_string();
    assert!(output.contains(":: renamed :: Arena"));
    assert!(output.contains(":: renamed :: de :: DeserializeIn"));
    assert!(!output.contains("wrong"));
}

#[test]
fn gamma_survivor_unit_output_matches_immutable_fixture() {
    let output = derive_deserialize_in(quote! { struct Unit; }, &parse_quote!(::multitude::de));
    let actual = syn::parse2::<syn::File>(output).unwrap();
    let expected = syn::parse_str::<syn::File>(include_str!("fixtures/gamma_survivor_unit.rs")).unwrap();
    assert_eq!(prettyplease::unparse(&actual), prettyplease::unparse(&expected));
}
