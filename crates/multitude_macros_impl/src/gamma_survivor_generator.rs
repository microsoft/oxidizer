// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

fn generated(source: &str) -> TokenStream2 {
    let input = syn::parse_str::<DeriveInput>(source).unwrap();
    let output = expand(&input, &parse_quote!(::multitude::de)).unwrap();
    syn::parse2::<syn::File>(output.clone()).unwrap();
    output
}

fn contains(output: &TokenStream2, expected: TokenStream2) {
    assert!(output.to_string().contains(&expected.to_string()), "missing {expected} in {output}");
}

#[test]
fn gamma_survivor_identifiers_and_lifetimes_do_not_collide() {
    let input: DeriveInput = parse_quote! {
        struct __A<'__arena, '__arena_, __Serde, __A_, const __A__: usize> {
            nested: Wrapper<(__Serde, __Serde_, __Serde__)>,
            literal: String,
        }
    };
    let names = internal_names(&input);
    assert_eq!(names.allocator.to_string(), "__A___");
    assert_eq!(names.serde.to_string(), "__Serde___");
    assert_eq!(fresh_lifetime(&input, "__arena").to_string(), "'__arena__");
    assert_eq!(fresh_lifetime(&input, "de").to_string(), "'de");
    let mut identifiers = HashSet::new();
    collect_identifiers(quote! { outer { (__A { [__Serde] }) } "__A_" + 1 }, &mut identifiers);
    assert_eq!(
        identifiers,
        HashSet::from(["outer".to_owned(), "__A".to_owned(), "__Serde".to_owned()])
    );
    assert_eq!(fresh_internal_ident(&identifiers, "__A").to_string(), "__A_");
    assert_eq!(fresh_internal_ident(&identifiers, "free").to_string(), "free");
    let output = generated("struct __A<__A_, __Serde, const __Serde_: usize>(__A_, [__Serde; __Serde_]);");
    contains(&output, quote! { use ::multitude::de::__private::serde as __Serde__; });
    contains(
        &output,
        quote! { __A__: ::multitude::de::__private::allocator_api2::alloc::Allocator + ::core::clone::Clone },
    );
    let input: DeriveInput = parse_quote!(
        struct Lifetime<'de, 'de_, '__arena>(&'de str, &'de_ str, &'__arena str);
    );
    assert_eq!(fresh_lifetime(&input, "de").to_string(), "'de__");
}

#[test]
fn gamma_survivor_bounds_preserve_order_flavor_and_defaults() {
    let input: DeriveInput = parse_quote! {
        struct Input<'a, T = u8, const N: usize = 2> where T: Clone {
            #[serde(skip)] skipped: Skip,
            first: T,
            repeated: T,
            #[multitude(via_serde)] serde_value: T,
            #[serde(default)] defaulted: T,
            #[serde(default)] repeated_default: T,
            #[multitude(deserialize_with = "decode")] custom: Custom,
            #[serde(deserialize_with = "serde_decode")] serde_custom: Custom,
            #[serde(skip, default = "make")] path_default: NoDefault,
            later: Later<'a, N>,
            plain_custom: Custom,
        }
    };
    let Data::Struct(data) = &input.data else {
        panic!("expected struct")
    };
    let infos = fields_info(&data.fields, None).unwrap();
    let names = internal_names(&input);
    let root = parse_quote!(::multitude::de);
    let de = parse_quote!('de);
    let bounds = bounded_generics(&input, &infos, &ContainerAttrs::default(), &de, &root, &names);
    let expected: Generics = parse_quote!(<'de, 'a, T, const N: usize, __A>);
    assert_eq!(bounds.params, expected.params);
    let expected: syn::WhereClause = parse_quote! {
        where T: Clone,
        __A: ::multitude::de::__private::allocator_api2::alloc::Allocator + ::core::clone::Clone,
        Skip: ::core::default::Default,
        T: ::multitude::de::DeserializeIn<'de, __A>,
        T: ::multitude::de::__private::serde::Deserialize<'de>,
        T: ::core::default::Default,
        Later<'a, N>: ::multitude::de::DeserializeIn<'de, __A>,
        Custom: ::multitude::de::DeserializeIn<'de, __A>
    };
    assert_eq!(bounds.where_clause.unwrap(), expected);
    let helper = helper_generics(&input.generics, &parse_quote!('__arena), &root, &names);
    assert_eq!(helper.params.first().unwrap().to_token_stream().to_string(), "'__arena");
    assert_eq!(helper.params.last().unwrap().to_token_stream().to_string(), "__A");
    assert_eq!(helper.where_clause.unwrap().predicates.len(), 2);
    assert_eq!(
        helper_impl_generics(&expected_generics(), &parse_quote!('__arena))
            .params
            .to_token_stream()
            .to_string(),
        "'de , '__arena , T , __A"
    );
    assert_eq!(arena_path(&root).to_token_stream().to_string(), ":: multitude :: Arena");
    let container = ContainerAttrs {
        deserialize_bounds: Some(vec![parse_quote!(T: Send)]),
        default: Some(DefaultValue::Trait),
        ..ContainerAttrs::default()
    };
    let bounded = bounded_generics(&input, &infos, &container, &de, &root, &names);
    let predicates = bounded.where_clause.unwrap().to_token_stream().to_string();
    assert_eq!(
        predicates,
        quote! { where T: Clone,
            __A: ::multitude::de::__private::allocator_api2::alloc::Allocator + ::core::clone::Clone,
            Input<'a, T, N>: ::core::default::Default,
            T: ::core::default::Default,
            T: Send
        }
        .to_string(),
    );
    let output = generated("enum Input<T> { #[serde(skip)] Hidden, Named { value: T } }");
    contains(&output, quote! { T: ::multitude::de::DeserializeIn<'de, __A> });
}

fn expected_generics() -> Generics {
    parse_quote!(<'de, T, __A>)
}

#[test]
fn gamma_survivor_seed_modes_and_indices() {
    let input: DeriveInput = parse_quote! {
        struct Input(
            u8,
            #[multitude(via_serde)] u16,
            #[serde(deserialize_with = "codec::decode")] u32,
            #[multitude(deserialize_with = "arena_decode")] u64,
        );
    };
    let Data::Struct(data) = &input.data else {
        panic!("expected struct")
    };
    let infos = fields_info(&data.fields, None).unwrap();
    let root = parse_quote!(::multitude::de);
    let names = internal_names(&input);
    let owner = format_ident!("Owner");
    let expected = [
        quote!(::multitude::de::DeserializeInSeed::<u8, __A>::new(self.arena)),
        quote!(::multitude::de::DeserializeSeed::<u16>::new()),
        quote!(OwnerField2Seed {
            arena: self.arena,
            marker: ::core::marker::PhantomData
        }),
        quote!(OwnerField3Seed {
            arena: self.arena,
            marker: ::core::marker::PhantomData
        }),
    ];
    let bounded = bounded_generics(&input, &infos, &ContainerAttrs::default(), &parse_quote!('de), &root, &names);
    let helper = helper_generics(&input.generics, &parse_quote!('__arena), &root, &names);
    for (index, (info, expected)) in infos.iter().zip(expected).enumerate() {
        assert_eq!(seed(info, &root, &owner, index, &names).to_string(), expected.to_string());
        let definition = custom_seed_definition(
            &input,
            info,
            index,
            &root,
            &parse_quote!('de),
            &parse_quote!('__arena),
            &bounded,
            &helper,
            &owner,
            &names,
        );
        if index < 2 {
            assert!(definition.is_none());
        } else {
            let definition = definition.unwrap();
            let method = if index == 2 {
                quote! { codec::decode(deserializer) }
            } else {
                quote! { arena_decode(self.arena, deserializer) }
            };
            contains(&definition, method);
            contains(&definition, quote! { arena: &'__arena ::multitude::Arena<__A> });
            contains(&definition, quote! { fn deserialize<__D>(self, deserializer: __D) });
        }
    }
    assert_eq!(custom_seed_name(&owner, 0).to_string(), "OwnerField0Seed");
    assert_eq!(custom_seed_name(&owner, 11).to_string(), "OwnerField11Seed");
    assert_eq!(
        custom_seed_name(&owner, usize::MAX).to_string(),
        format!("OwnerField{}Seed", usize::MAX)
    );
}

#[test]
fn gamma_survivor_custom_variant_construction() {
    let input: DeriveInput = parse_quote! {
        enum Input {
            Unit,
            EmptyTuple(),
            Single(u8),
            Pair(u8, u16),
            EmptyRecord {},
            SingleRecord { first: u8 },
            Record { first: u8, second: u16, third: u32 },
        }
    };
    let Data::Enum(data) = &input.data else { panic!("expected enum") };
    let expected = [
        quote! { let _: () = __value; Input::Unit },
        quote! { let _: () = __value; Input::EmptyTuple() },
        quote! { Input::Single(__value) },
        quote! { Input::Pair(__value.0, __value.1) },
        quote! { let _: () = __value; Input::EmptyRecord {} },
        quote! { Input::SingleRecord { first: __value } },
        quote! { Input::Record { first: __value.0, second: __value.1, third: __value.2 } },
    ];
    for (variant, expected) in data.variants.iter().zip(expected) {
        assert_eq!(
            construct_custom_variant(&input.ident, &variant.ident, &variant.fields).to_string(),
            expected.to_string()
        );
    }
    for source in [
        r#"enum Input { #[serde(deserialize_with = "decode")] Unit }"#,
        r#"enum Input { #[serde(deserialize_with = "decode")] New(u8) }"#,
        r#"enum Input { #[serde(deserialize_with = "decode")] Pair(u8, u16) }"#,
        r#"enum Input { #[multitude(deserialize_with = "decode")] Named { value: u8 } }"#,
    ] {
        let output = generated(source);
        contains(&output, quote! { __MultitudeVisitorForInputVariant0Seed });
        assert_eq!(output.to_string().matches("newtype_variant_seed").count(), 1);
        assert!(!output.to_string().contains("tuple_variant"));
        assert!(!output.to_string().contains("struct_variant"));
        if source.contains("multitude") {
            contains(&output, quote! { type Value = u8; });
            contains(&output, quote! { decode(self.arena, deserializer) });
        } else {
            contains(&output, quote! { decode(deserializer) });
        }
    }
}

#[test]
fn gamma_survivor_field_and_variant_name_tables() {
    let input: DeriveInput = parse_quote! {
        struct Input {
            #[serde(rename = "first", alias = "old")] first_value: u8,
            #[serde(skip, alias = "omitted")] hidden: u8,
            last_value: u16,
        }
    };
    let Data::Struct(data) = &input.data else {
        panic!("expected struct")
    };
    let infos = fields_info(&data.fields, Some(RenameRule::Camel)).unwrap();
    assert_eq!(accepted_field_names(&infos), ["first", "old", "lastValue"]);
    let name = format_ident!("Keys");
    let names = internal_names(&input);
    let strict = field_enum(&name, &infos, true, &names).unwrap();
    contains(&strict, quote! { enum Keys { Field0, Field2, Ignore, } });
    contains(&strict, quote! { const Keys_FIELDS: &[&str] = &["first", "old", "lastValue"]; });
    for (wire, bytes, target) in [
        (quote!("first"), quote!(b"first"), quote!(Keys::Field0)),
        (quote!("old"), quote!(b"old"), quote!(Keys::Field0)),
        (quote!("lastValue"), quote!(b"lastValue"), quote!(Keys::Field2)),
    ] {
        contains(&strict, quote! { #wire => ::core::result::Result::Ok(#target) });
        contains(&strict, quote! { #bytes => ::core::result::Result::Ok(#target) });
    }
    assert!(!strict.to_string().contains("\"hidden\""));
    assert!(!strict.to_string().contains("\"omitted\""));
    contains(&strict, quote! { 0u64 => ::core::result::Result::Ok(Keys::Field0) });
    contains(&strict, quote! { 1u64 => ::core::result::Result::Ok(Keys::Field2) });
    contains(&strict, quote! { &"field index 0 <= i < 2" });
    contains(&strict, quote! { __E::unknown_field(__value, Keys_FIELDS) });
    contains(&strict, quote! { __Serde::de::Unexpected::Bytes(__value) });
    let permissive = field_enum(&name, &infos, false, &names).unwrap();
    contains(&permissive, quote! { "hidden" => ::core::result::Result::Ok(Keys::Ignore) });
    contains(&permissive, quote! { b"omitted" => ::core::result::Result::Ok(Keys::Ignore) });
    assert_eq!(
        permissive
            .to_string()
            .matches("_ => :: core :: result :: Result :: Ok (Keys :: Ignore)")
            .count(),
        3
    );
    assert!(!permissive.to_string().contains("unknown_field"));
    assert!(!permissive.to_string().contains("Unexpected"));
    let output = generated("enum Input { First, #[serde(skip)] Hidden, #[serde(alias = \"old\")] LastValue }");
    contains(&output, quote! { enum __MultitudeVariantForInput { Variant0, Variant2, } });
    contains(
        &output,
        quote! { const __MultitudeVariantForInput_VARIANTS: &[&str] = &["First", "LastValue", "old"]; },
    );
    contains(
        &output,
        quote! { 1u64 => ::core::result::Result::Ok(__MultitudeVariantForInput::Variant2) },
    );
    contains(
        &output,
        quote! { b"old" => ::core::result::Result::Ok(__MultitudeVariantForInput::Variant2) },
    );
    assert!(!output.to_string().contains("\"Hidden\""));
    contains(&output, quote! { &"variant index 0 <= i < 2" });
}

#[test]
fn gamma_survivor_shape_and_default_output() {
    let cases = [
        (
            "struct Unit;",
            quote! { formatter.write_str("a unit value") },
            quote! { ::core::result::Result::Ok(Unit) },
        ),
        (
            "struct New(u8);",
            quote! { formatter.write_str("a newtype value") },
            quote! { New(__value) },
        ),
        (
            "struct Named { value: u8 }",
            quote! { formatter.write_str("a struct") },
            quote! { let mut __field0 = ::core::option::Option::None; },
        ),
        (
            "enum Input { Unit }",
            quote! { formatter.write_str("an externally tagged enum") },
            quote! { ::core::result::Result::Ok(Input::Unit) },
        ),
        (
            "#[serde(transparent)] struct Transparent(#[serde(skip)] u8, u16);",
            quote! { formatter.write_str("a transparent value") },
            quote! { ::multitude::de::DeserializeInSeed::<u16, __A>::new(self.arena) },
        ),
    ];
    for (source, expectation, body) in cases {
        let output = generated(source);
        contains(&output, expectation);
        contains(&output, body);
    }
    for (source, seed_name) in [
        (
            r#"struct New(#[serde(deserialize_with = "decode")] u8);"#,
            "__MultitudeVisitorForNewField0Seed",
        ),
        (
            r#"struct Pair(#[serde(skip)] u8, #[multitude(deserialize_with = "decode")] u16);"#,
            "__MultitudeVisitorForPairField1Seed",
        ),
        (
            r#"#[serde(transparent)] struct New(#[serde(skip)] u8, #[serde(deserialize_with = "decode")] u16);"#,
            "__MultitudeVisitorForNewField1Seed",
        ),
        (
            r#"enum Input { New(#[serde(deserialize_with = "decode")] u8), Pair(u8, u16) }"#,
            "__MultitudeVisitorForInputVariant0Field0Seed",
        ),
    ] {
        let output = generated(source);
        let seed_name = Ident::new(seed_name, Span::call_site());
        contains(&output, quote! { decode });
        contains(&output, quote! { struct #seed_name });
        contains(
            &output,
            quote! { #seed_name { arena: self.arena, marker: ::core::marker::PhantomData } },
        );
        assert!(output.to_string().contains("Seed"));
        assert!(output.to_string().contains("fn deserialize"));
    }
    let output =
        generated(r#"#[serde(default = "make")] struct Input { first: u8, #[serde(skip)] hidden: u8, #[serde(default)] last: u16 }"#);
    contains(&output, quote! { first: __container_default0 });
    contains(&output, quote! { hidden: __container_default1 });
    assert!(!output.to_string().contains("__container_default2"));
    contains(&output, quote! { let __field1 = match __container_default1 });
    contains(&output, quote! { last: match __field2 });
    let output = generated("struct AllSkipped { #[serde(skip)] hidden: u8 }");
    assert!(!output.to_string().contains("mut __seq"));
    let output = generated("struct Pair(#[serde(skip)] u8, u16, u32);");
    contains(&output, quote! { missing_field("1") });
    contains(&output, quote! { missing_field("2") });
    contains(
        &output,
        quote! {
            deserialize_tuple_struct(deserializer, "Pair", 2usize,
                __MultitudeVisitorForPair { arena, marker: ::core::marker::PhantomData })
        },
    );
    let output = generated("struct Record { #[serde(skip)] hidden: u8, first: u16, second: u32 }");
    contains(&output, quote! { invalid_length(0usize, &self) });
    contains(&output, quote! { invalid_length(1usize, &self) });
    let output = generated(r#"#[serde(expecting = "exact expectation", rename = "Wire")] struct Input { field: u8 }"#);
    contains(&output, quote! { formatter.write_str("exact expectation") });
    contains(&output, quote! { "Wire" });
    let output = generated("enum Input { #[serde(skip, default)] Hidden, Ready }");
    assert!(!output.to_string().contains("\"Hidden\""));
    contains(&output, quote! { "Ready" });
}
