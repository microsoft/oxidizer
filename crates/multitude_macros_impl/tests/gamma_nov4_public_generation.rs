// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public derive entrypoint oracles for complete generated visitors.

use multitude_macros_impl::derive_deserialize_in;
use proc_macro2::TokenStream;
use quote::{ToTokens as _, quote};
use syn::parse_quote;
use syn::visit::Visit as _;

#[derive(Default)]
struct StringArms(Vec<(String, String)>);

impl<'ast> syn::visit::Visit<'ast> for StringArms {
    fn visit_arm(&mut self, i: &'ast syn::Arm) {
        if let syn::Pat::Lit(syn::ExprLit {
            lit: syn::Lit::Str(name), ..
        }) = &i.pat
        {
            self.0.push((name.value(), i.body.to_token_stream().to_string()));
        }
        syn::visit::visit_arm(self, i);
    }
}

#[test]
fn field_and_variant_identifiers_emit_string_arms_not_just_byte_string_arms() {
    for (input, target) in [
        (
            quote! {
                struct Packet {
                    #[serde(rename = "wire", alias = "legacy")]
                    value: u8,
                }
            },
            quote!(::core::result::Result::Ok(__MultitudeFieldForPacket::Field0)),
        ),
        (
            quote! {
                enum Packet {
                    #[serde(rename = "wire", alias = "legacy")]
                    Value,
                }
            },
            quote!(::core::result::Result::Ok(__MultitudeVariantForPacket::Variant0)),
        ),
    ] {
        let output = derive_deserialize_in(input, &parse_quote!(::multitude::de));
        let mut arms = StringArms::default();
        arms.visit_file(&syn::parse2(output).unwrap());
        assert_eq!(arms.0, [("wire".into(), target.to_string()), ("legacy".into(), target.to_string())]);
    }
}

fn generated(input: TokenStream) -> String {
    let output = derive_deserialize_in(input, &parse_quote!(::multitude::de));
    syn::parse2::<syn::File>(output.clone()).expect("valid derive fixtures must generate syntactically valid Rust");
    output.to_string().split_whitespace().collect()
}

fn contains(output: &str, expected: &TokenStream) {
    let expected = expected.to_string().split_whitespace().collect::<String>();
    assert!(output.contains(&expected), "missing {expected} in {output}");
}

#[test]
fn gamma_nov6_byte_identifier_visitors_retain_each_field_and_variant_alias() {
    let fields = generated(quote! {
        struct Packet {
            #[serde(rename = "wire", alias = "legacy")]
            value: u8,
        }
    });
    contains(
        &fields,
        &quote!(b"wire" => ::core::result::Result::Ok(__MultitudeFieldForPacket::Field0)),
    );
    contains(
        &fields,
        &quote!(b"legacy" => ::core::result::Result::Ok(__MultitudeFieldForPacket::Field0)),
    );
    let variants = generated(quote! {
        enum Packet {
            #[serde(rename = "wire", alias = "legacy")]
            Value,
        }
    });
    contains(
        &variants,
        &quote!(b"wire" => ::core::result::Result::Ok(__MultitudeVariantForPacket::Variant0)),
    );
    contains(
        &variants,
        &quote!(b"legacy" => ::core::result::Result::Ok(__MultitudeVariantForPacket::Variant0)),
    );
}

#[test]
fn gamma_nov4_conflicting_field_modes_report_the_first_attribute_source() {
    for (attributes, first) in [
        (
            r#"#[serde(deserialize_with = "decode")] #[multitude(via_serde)]"#,
            r#"#[serde(deserialize_with = "decode")]"#,
        ),
        (r"#[serde(skip)] #[multitude(via_serde)]", r"#[serde(skip)]"),
    ] {
        let input = format!("struct Packet {{ {attributes} value: u8 }}")
            .parse()
            .expect("the conflicting attribute fixture is valid Rust token syntax");
        let output = derive_deserialize_in(input, &parse_quote!(::multitude::de));
        assert!(output.to_string().contains("compile_error"));
        let mut tokens = output.into_iter();
        let start = tokens.next().expect("a diagnostic contains a compile_error invocation").span();
        let end = tokens.last().expect("a diagnostic contains more than one token").span();
        let source = start
            .join(end)
            .expect("the diagnostic endpoints originate in the same parsed attribute")
            .source_text()
            .expect("the source of parsed attribute tokens remains available");
        assert_eq!(source, first);
    }
}

#[test]
fn gamma_nov4_public_named_visitor_keeps_both_default_sources_and_all_wire_names() {
    let output = generated(quote! {
        #[serde(default = "defaults", deny_unknown_fields)]
        struct Packet {
            #[serde(rename = "wire_a", alias = "old_a", default = "field_default")]
            a: u16,
            #[serde(skip)]
            ignored: u8,
            b: u32,
        }
    });
    contains(
        &output,
        &quote!(
            const __MultitudeFieldForPacket_FIELDS: &[&str] = &["wire_a", "old_a", "b"];
        ),
    );
    contains(
        &output,
        &quote!("wire_a" => ::core::result::Result::Ok(__MultitudeFieldForPacket::Field0)),
    );
    contains(
        &output,
        &quote!("old_a" => ::core::result::Result::Ok(__MultitudeFieldForPacket::Field0)),
    );
    contains(&output, &quote!(__E::unknown_field(__value, __MultitudeFieldForPacket_FIELDS)));
    contains(
        &output,
        &quote!(let Packet { ignored: __container_default1, b: __container_default2, .. } = defaults();),
    );
    contains(
        &output,
        &quote! {
            ignored: match __container_default1 {
                ::core::option::Option::Some(__value) => __value,
                ::core::option::Option::None => ::core::unreachable!(),
            }
        },
    );
    contains(
        &output,
        &quote! {
            b: match __field2 {
                ::core::option::Option::Some(__value) => __value,
                ::core::option::Option::None => match __container_default2 {
                    ::core::option::Option::Some(__value) => __value,
                    ::core::option::Option::None => ::core::unreachable!(),
                },
            }
        },
    );
    contains(
        &output,
        &quote! {
            let __field1 = match __container_default1 {
                ::core::option::Option::Some(__value) => __value,
                ::core::option::Option::None => ::core::unreachable!(),
            };
        },
    );
    contains(
        &output,
        &quote! {
            let __field2 = match __Serde::de::SeqAccess::next_element_seed(
                &mut __seq,
                ::multitude::de::DeserializeInSeed::<u32, __A>::new(self.arena)
            )? {
                ::core::option::Option::Some(__value) => __value,
                ::core::option::Option::None => match __container_default2 {
                    ::core::option::Option::Some(__value) => __value,
                    ::core::option::Option::None => ::core::unreachable!(),
                },
            };
        },
    );
    assert!(
        output.contains("Deserializer::deserialize_struct(deserializer,\"Packet\",&[\"wire_a\",\"old_a\",\"b\"],"),
        "{output}"
    );
    assert!(output.contains("fnvisit_seq<__S>(self,mut__seq:__S)"), "{output}");
    assert!(!output.contains("__container_default0"), "{output}");
    assert!(!output.contains("letmut__field1="), "{output}");
    assert_eq!(
        output
            .matches("__E::unknown_field(__value,__MultitudeFieldForPacket_FIELDS)")
            .count(),
        2,
        "{output}"
    );
    assert!(output.contains("::core::option::Option::None=>field_default()"), "{output}");
    assert!(
        output.contains("::core::option::Option::None=>match__container_default2"),
        "{output}"
    );
}

#[test]
fn gamma_nov4_public_skipped_fields_do_not_request_values_or_container_defaults() {
    let output = generated(quote! {
        struct Plain {
            #[serde(skip)]
            ignored: u8,
            kept: u16,
        }
    });
    contains(&output, &quote!(ignored: ::core::default::Default::default()));
    contains(&output, &quote!(let __field0 = ::core::default::Default::default();));
    assert!(!output.contains("__container_default"), "{output}");
    assert!(output.contains("fnvisit_seq<__S>(self,mut__seq:__S)"), "{output}");
    let tuple = generated(quote! {
        struct Tuple(#[serde(skip)] u8, u16);
    });
    assert!(tuple.contains("Ok(Tuple(::core::default::Default::default(),match"), "{tuple}");
    assert!(
        tuple.contains("Deserializer::deserialize_tuple_struct(deserializer,\"Tuple\",1usize,"),
        "{tuple}"
    );
    let skipped = generated(quote! {
        struct Skipped {
            #[serde(skip)]
            first: u8,
            #[serde(skip)]
            second: u16,
        }
    });
    assert!(skipped.contains("fnvisit_seq<__S>(self,_:__S)"), "{skipped}");
    assert!(!skipped.contains("SeqAccess::next_element_seed"), "{skipped}");
}

#[test]
fn gamma_nov4_public_enum_retains_payload_visitor_definitions_and_deserializer_name_lists() {
    let output = generated(quote! {
        enum Choices {
            Named {
                #[serde(alias = "old_x")]
                x: u8,
            },
            Pair(u16, u32),
            Unit,
        }
    });
    contains(
        &output,
        &quote!("Named" => ::core::result::Result::Ok(__MultitudeVariantForChoices::Variant0)),
    );
    assert!(output.contains("struct__MultitudeVisitorForChoicesVariant1"), "{output}");
    assert!(
        output.contains("VariantAccess::struct_variant(__access,&[\"x\",\"old_x\"],"),
        "{output}"
    );
    assert!(
        output.contains("Deserializer::deserialize_enum(deserializer,\"Choices\",&[\"Named\",\"Pair\",\"Unit\"],"),
        "{output}"
    );
    assert!(output.contains("VariantAccess::tuple_variant(__access,2usize,"), "{output}");
}
