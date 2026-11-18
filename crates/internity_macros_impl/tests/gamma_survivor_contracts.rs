// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process derive contract tests.

use internity_macros_impl::{derive_deserialize_in, derive_serialize_in};
use proc_macro2::TokenStream;
use quote::quote;
use syn::parse_quote;

#[test]
fn gamma_round2_unknown_byte_names_and_numeric_indices_are_ignored_by_default() {
    let output = derive_deserialize_in(
        quote!(
            struct Example {
                value: String,
            }
        ),
        &parse_quote!(::internity),
    );
    let text = output.to_string();
    assert_eq!(
        text.matches(&quote!(_ => __InternityFieldForExample::__ignore).to_string()).count(),
        2
    );
    assert!(!text.contains("unknown_field"));
    assert!(!text.contains("invalid_value"));
    syn::parse2::<syn::File>(output).unwrap();
}

#[cfg(test)]
fn diagnostic(input: TokenStream, serialize: bool) -> String {
    let root = parse_quote!(::internity);
    let output = if serialize {
        derive_serialize_in(input, &root)
    } else {
        derive_deserialize_in(input, &root)
    };
    let item: syn::ItemMacro = syn::parse2(output).unwrap();
    assert_eq!(item.mac.path.segments.last().unwrap().ident, "compile_error");
    syn::parse2::<syn::LitStr>(item.mac.tokens).unwrap().value()
}

#[test]
fn gamma_survivor_field_diagnostics() {
    for (attrs, expected) in [
        (quote!(#[serde(rename = "a", rename = "b")]), "duplicate serde rename"),
        (
            quote!(#[serde(rename(deserialize = "a", deserialize = "b"))]),
            "duplicate serde deserialize rename",
        ),
        (
            quote!(#[serde(rename(serialize = "a", serialize = "b"))]),
            "duplicate serde serialize rename",
        ),
        (quote!(#[serde(rename(other = "a"))]), "expected `serialize` or `deserialize`"),
        (quote!(#[internity(via_serde, via_serde)]), "duplicate `via_serde`"),
        (
            quote!(#[internity(other)]),
            "unsupported internity field attribute; expected `via_serde`",
        ),
        (quote!(#[serde(default, default)]), "duplicate default"),
        (
            quote!(#[serde(flatten)]),
            "serde `flatten` is not supported by internity's `DeserializeIn`",
        ),
        (
            quote!(#[serde(borrow)]),
            "serde `borrow` is not supported by internity's `DeserializeIn`",
        ),
        (
            quote!(#[serde(getter)]),
            "serde `getter` is only valid with `remote`, which internity's derives do not support",
        ),
        (
            quote!(#[serde(other)]),
            "unsupported serde field attribute for internity's `DeserializeIn`",
        ),
        (
            quote!(#[serde(deserialize_with = "a", deserialize_with = "b")]),
            "duplicate serde deserializer",
        ),
        (quote!(#[serde(serialize_with = "a", with = "b")]), "duplicate serde serializer"),
        (quote!(#[serde(with = "a", with = "b")]), "duplicate serde serializer"),
    ] {
        for serialize in [false, true] {
            assert_eq!(
                diagnostic(quote!(struct Example { #attrs field: u32 }), serialize),
                expected,
                "{attrs}"
            );
        }
    }
}

#[test]
fn gamma_survivor_container_diagnostics() {
    for (attrs, expected) in [
        (quote!(#[serde(rename_all = "unknown")]), "unknown serde rename rule"),
        (quote!(#[serde(rename_all(deserialize = "unknown"))]), "unknown serde rename rule"),
        (quote!(#[serde(rename_all(serialize = "unknown"))]), "unknown serde rename rule"),
        (
            quote!(#[serde(rename_all = "camelCase", rename_all = "snake_case")]),
            "duplicate serde rename rule",
        ),
        (
            quote!(#[serde(rename_all(deserialize = "camelCase", deserialize = "snake_case"))]),
            "duplicate serde deserialize rename rule",
        ),
        (
            quote!(#[serde(rename_all(serialize = "camelCase", serialize = "snake_case"))]),
            "duplicate serde serialize rename rule",
        ),
        (
            quote!(#[serde(rename_all(other = "camelCase"))]),
            "expected `serialize` or `deserialize`",
        ),
        (quote!(#[internity(crate = "a", crate = "b")]), "duplicate `internity(crate = ...)`"),
        (
            quote!(#[internity(other = "a")]),
            "unsupported internity container attribute; expected `crate = \"...\"`",
        ),
        (quote!(#[serde(default, default)]), "duplicate serde container default"),
        (quote!(#[serde(transparent, transparent)]), "duplicate serde `transparent`"),
        (
            quote!(#[serde(expecting = "a", expecting = "b")]),
            "duplicate serde container expectation",
        ),
        (
            quote!(#[serde(rename_all_fields)]),
            "serde `rename_all_fields` applies to enums, which internity's `DeserializeIn` does not support",
        ),
    ] {
        for serialize in [false, true] {
            assert_eq!(
                diagnostic(quote!(#attrs struct Example { field: u32 }), serialize),
                expected,
                "{attrs}"
            );
        }
    }
}

#[test]
fn gamma_survivor_expansion_validation() {
    for serialize in [false, true] {
        let direction = if serialize { "SerializeIn" } else { "DeserializeIn" };
        assert_eq!(
            diagnostic(
                quote!(
                    struct Example<T>(T);
                ),
                serialize
            ),
            format!("internity::{direction} cannot be derived for generic types")
        );
        assert_eq!(
            diagnostic(
                quote!(
                    enum Example {
                        A,
                    }
                ),
                serialize
            ),
            format!("internity::{direction} can only be derived for structs")
        );
        assert_eq!(
            diagnostic(
                quote!(
                    #[serde(transparent)]
                    struct Example(u32, u32);
                ),
                serialize
            ),
            "serde `transparent` requires a single-field tuple struct or a struct with exactly one transparent field"
        );
        let expected = if serialize {
            "serde `transparent` requires exactly one field that is not skipped serializing or PhantomData"
        } else {
            "serde `transparent` requires exactly one field that is not skipped, defaulted, or PhantomData"
        };
        for input in [
            quote!(
                #[serde(transparent)]
                struct Example {
                    a: u32,
                    b: u32,
                }
            ),
            quote!(
                #[serde(transparent)]
                struct Example(#[serde(skip)] u32);
            ),
        ] {
            assert_eq!(diagnostic(input, serialize), expected);
        }
    }
    assert_eq!(
        diagnostic(
            quote!(
                #[serde(default)]
                struct Example;
            ),
            false
        ),
        "serde container `default` can only be used on structs that have fields"
    );
    assert_eq!(
        diagnostic(
            quote!(
                #[serde(from = "u32")]
                struct Example(u32);
            ),
            false
        ),
        "serde `from` changes how a value is deserialized, which internity's `DeserializeIn` cannot honor"
    );
    assert_eq!(
        diagnostic(
            quote!(
                #[serde(try_from = "u32")]
                struct Example(u32);
            ),
            false
        ),
        "serde `try_from` changes how a value is deserialized, which internity's `DeserializeIn` cannot honor"
    );
    assert_eq!(
        diagnostic(
            quote!(
                #[serde(into = "u32")]
                struct Example(u32);
            ),
            true
        ),
        "serde `into` changes how a value is serialized, which internity's `SerializeIn` cannot honor"
    );
}

#[test]
fn gamma_survivor_rename_styles_and_wire_indices() {
    let root = parse_quote!(::internity);
    for (style, name) in [
        ("lowercase", "a_bc"),
        ("UPPERCASE", "A_BC"),
        ("PascalCase", "ABc"),
        ("camelCase", "aBc"),
        ("snake_case", "a_bc"),
        ("SCREAMING_SNAKE_CASE", "A_BC"),
        ("kebab-case", "a-bc"),
        ("SCREAMING-KEBAB-CASE", "A-BC"),
    ] {
        let input = quote!(
            #[serde(rename_all = #style)]
            struct Example {
                a_bc: u32,
            }
        );
        for output in [derive_deserialize_in(input.clone(), &root), derive_serialize_in(input, &root)] {
            syn::parse2::<syn::File>(output.clone()).unwrap();
            assert!(output.to_string().contains(&format!("\"{name}\"")));
        }
    }
    let output = derive_deserialize_in(
        quote!(
            struct Example {
                #[serde(alias = "alias_a")]
                a: u32,
                #[serde(skip)]
                skipped: u32,
                b: u32,
            }
        ),
        &root,
    )
    .to_string();
    for expected in [
        quote!(b"a" => __InternityFieldForExample::__field0),
        quote!(b"alias_a" => __InternityFieldForExample::__field0),
        quote!(b"b" => __InternityFieldForExample::__field1),
        quote!(0 => __InternityFieldForExample::__field0),
        quote!(1 => __InternityFieldForExample::__field1),
        quote!(invalid_length(0, &self)),
        quote!(invalid_length(1, &self)),
        quote!(__map.next_value::<::internity::de::__private::serde::de::IgnoredAny>()?),
        quote!(let __internity_value_0 = match __internity_slot_0),
    ] {
        assert!(output.contains(&expected.to_string()), "{expected}");
    }
    assert!(!output.contains(&quote!(2 => __InternityFieldForExample::__field2).to_string()));
}

#[test]
fn gamma_survivor_hygienic_helper_collisions() {
    let root = parse_quote!(::internity);
    let output = derive_deserialize_in(
        quote!(
            struct Example {
                __InternityInternerForExample: u32,
                __InternityInternerForExample_: u32,
                __InternityVisitorForExample: u32,
            }
        ),
        &root,
    );
    syn::parse2::<syn::File>(output.clone()).unwrap();
    assert!(output.to_string().contains("__InternityInternerForExample__"));
    assert!(output.to_string().contains("struct __InternityVisitorForExample_"));
}

#[test]
fn gamma_survivor_tuple_skip_default_and_newtype_shape() {
    let root = parse_quote!(::internity);
    let input = quote!(
        struct Example(#[serde(skip)] u32, u32);
    );
    let deserialize = derive_deserialize_in(input.clone(), &root).to_string();
    let serialize = derive_serialize_in(input, &root).to_string();
    assert!(!deserialize.contains("visit_newtype_struct"));
    assert!(serialize.contains("serialize_tuple_struct"));
    assert!(!serialize.contains("serialize_newtype_struct"));
    let skipped = derive_serialize_in(
        quote!(
            struct Example(#[serde(skip_serializing)] u32);
        ),
        &root,
    )
    .to_string();
    assert!(skipped.contains("serialize_tuple_struct"));
    assert!(!skipped.contains("serialize_newtype_struct"));
    assert!(deserialize.contains(&quote!(invalid_length(0, &self)).to_string()));
    assert!(deserialize.contains(&quote!(let __value0 = ::core::default::Default::default()).to_string()));
    let output = derive_deserialize_in(
        quote!(
            struct Example(#[serde(skip)] u32, #[serde(default)] u32, u32);
        ),
        &root,
    );
    assert_eq!(
        diagnostic(
            quote!(
                struct Example(#[serde(skip)] u32, #[serde(default)] u32, u32);
            ),
            false
        ),
        "field must have #[serde(default)] because previous field 1 has #[serde(default)]"
    );
    assert!(output.to_string().contains("compile_error"));
}

#[test]
fn gamma_survivor_custom_seed_and_conflicting_modes() {
    let root = parse_quote!(::internity);
    let output = derive_deserialize_in(
        quote!(
            struct Example {
                #[serde(deserialize_with = "custom")]
                a: u32,
            }
        ),
        &root,
    )
    .to_string();
    assert!(output.contains("custom (__d)"));
    assert!(!output.contains("DeserializeInSeed :: < u32"));
    for input in [
        quote!(
            struct Example {
                #[internity(via_serde)]
                #[serde(deserialize_with = "custom")]
                a: u32,
            }
        ),
        quote!(
            struct Example(
                #[internity(via_serde)]
                #[serde(deserialize_with = "custom")]
                u32,
            );
        ),
    ] {
        assert_eq!(
            diagnostic(input, false),
            "`#[internity(via_serde)]` and serde `deserialize_with`/`with` are mutually exclusive for `DeserializeIn`"
        );
    }
    assert_eq!(
        diagnostic(
            quote!(
                struct Example {
                    #[serde(skip, deserialize_with = "custom")]
                    a: u32,
                }
            ),
            false
        ),
        "serde `skip`/`skip_deserializing` cannot be combined with a custom deserializer mode (`via_serde`/`deserialize_with`/`with`)"
    );
    assert_eq!(
        diagnostic(
            quote!(
                struct Example(
                    #[internity(via_serde)]
                    #[serde(serialize_with = "custom")]
                    u32,
                );
            ),
            true
        ),
        "`#[internity(via_serde)]` and serde `serialize_with`/`with` are mutually exclusive for `SerializeIn`"
    );
}

#[test]
fn gamma_survivor_transparent_skipped_field_uses_its_own_default() {
    let root = parse_quote!(::internity);
    let output = derive_deserialize_in(
        quote!(
            #[serde(transparent)]
            struct Example {
                a: u32,
                #[serde(skip, default = "custom")]
                b: u32,
            }
        ),
        &root,
    )
    .to_string();
    assert!(output.contains("b : custom ()"));
    assert!(!output.contains("compile_error"));
    let input = quote!(
        #[serde(default = "container")]
        struct Example(#[serde(skip)] u32);
    );
    let output = derive_deserialize_in(input, &root).to_string();
    assert!(!output.contains("container ()"));
    assert!(output.contains("Default :: default ()"));
    let output = derive_deserialize_in(
        quote!(
            #[serde(default = "container")]
            struct Example(#[serde(skip)] u32, u32);
        ),
        &root,
    )
    .to_string();
    assert!(output.contains("container ()"));
}

#[test]
fn gamma_survivor_missing_attribute_values_return_diagnostics() {
    for attrs in [
        quote!(#[serde(rename(deserialize))]),
        quote!(#[serde(rename(serialize))]),
        quote!(#[serde(alias)]),
        quote!(#[serde(with)]),
        quote!(#[serde(deserialize_with)]),
        quote!(#[serde(serialize_with)]),
        quote!(#[serde(skip_serializing_if)]),
    ] {
        assert_eq!(diagnostic(quote!(struct Example { #attrs a: u32 }), false), "expected `=`");
    }

    for attrs in [
        quote!(#[serde(rename_all(deserialize))]),
        quote!(#[serde(rename_all(serialize))]),
        quote!(#[internity(crate)]),
        quote!(#[serde(expecting)]),
        quote!(#[serde(from)]),
        quote!(#[serde(try_from)]),
        quote!(#[serde(into)]),
        quote!(#[serde(bound(deserialize))]),
    ] {
        assert_eq!(diagnostic(quote!(#attrs struct Example { a: u32 }), false), "expected `=`");
    }
    for attrs in [
        quote!(#[serde(from = 3)]),
        quote!(#[serde(try_from = 3)]),
        quote!(#[serde(into = 3)]),
    ] {
        assert_eq!(
            diagnostic(quote!(#attrs struct Example { a: u32 }), false),
            "expected string literal"
        );
    }
}
