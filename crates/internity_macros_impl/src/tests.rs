// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::{DeriveInput, Field, parse_quote};

use super::{default_root, expand_deserialize, expand_serialize, field_seed, parse_container, parse_field, resolve_de_root};

fn compact(tokens: impl quote::ToTokens) -> String {
    tokens.to_token_stream().to_string().replace(' ', "")
}

fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    expand_deserialize(input, &default_root())
}

fn serialize(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    expand_serialize(input, &default_root())
}

#[test]
fn parses_container_configuration() {
    let default_input: DeriveInput = parse_quote!(
        #[derive(Clone)]
        struct Example;
    );
    assert_eq!(compact(default_root()), "::internity");
    assert_eq!(
        compact(resolve_de_root(&parse_container(&default_input.attrs).unwrap(), &default_root())),
        "::internity::de"
    );

    let custom_input: DeriveInput = parse_quote!(
        #[internity(crate = "renamed")]
        struct Example;
    );
    assert_eq!(
        compact(resolve_de_root(&parse_container(&custom_input.attrs).unwrap(), &default_root())),
        "renamed::de"
    );

    let invalid_input: DeriveInput = parse_quote!(
        #[internity(other)]
        struct Example;
    );
    assert!(
        parse_container(&invalid_input.attrs).is_err(),
        "unsupported container attribute must fail"
    );
}

#[test]
fn parses_field_configuration() {
    let ordinary: Field = parse_quote!(
        #[doc = "ignored"]
        value: String
    );
    assert!(!parse_field(&ordinary.attrs).unwrap().via_serde);

    let via_serde: Field = parse_quote!(
        #[internity(via_serde)]
        value: String
    );
    assert!(parse_field(&via_serde.attrs).unwrap().via_serde);

    let invalid: Field = parse_quote!(
        #[internity(other)]
        value: String
    );
    assert!(parse_field(&invalid.attrs).is_err(), "unsupported field attribute must fail");
}

#[test]
fn selects_the_requested_field_seed() {
    let root = default_root();
    let root = super::append_module(&root, parse_quote!(de));
    let interner = parse_quote!(__Interner);
    let ty = parse_quote!(String);
    let access = quote!(self.__internity_interner);

    let via_serde_field: Field = parse_quote!(#[internity(via_serde)] value: String);
    let via_serde = parse_field(&via_serde_field.attrs).unwrap();
    assert_eq!(
        compact(field_seed(&root, &interner, &ty, &via_serde, None, &access)),
        "::internity::de::DeserializeSeed::<String>::new()"
    );

    let plain_field: Field = parse_quote!(value: String);
    let plain = parse_field(&plain_field.attrs).unwrap();
    assert_eq!(
        compact(field_seed(&root, &interner, &ty, &plain, None, &access)),
        "::internity::de::DeserializeInSeed::<String,__Interner>::new(&mut*self.__internity_interner)"
    );
}

#[test]
fn expands_named_tuple_and_unit_structs() {
    let named: DeriveInput = parse_quote! {
        struct Named {
            symbol: Sym,
            #[internity(via_serde)]
            count: u32,
        }
    };
    let named = compact(expand(&named).unwrap());
    assert!(named.contains("deserialize_struct"));
    assert!(named.contains("DeserializeInSeed::<Sym"));
    assert!(named.contains("DeserializeSeed::<u32>"));

    let newtype: DeriveInput = parse_quote!(
        struct Newtype(Sym);
    );
    let newtype = compact(expand(&newtype).unwrap());
    assert!(newtype.contains("deserialize_newtype_struct"));
    assert!(newtype.contains("visit_newtype_struct"));
    assert!(!newtype.contains("deserialize_tuple_struct"));

    let pair: DeriveInput = parse_quote!(
        struct Pair(Sym, u32);
    );
    let pair = compact(expand(&pair).unwrap());
    assert!(pair.contains("deserialize_tuple_struct"));
    assert!(!pair.contains("visit_newtype_struct"));

    let unit: DeriveInput = parse_quote!(
        struct Unit;
    );
    let unit = compact(expand(&unit).unwrap());
    assert!(unit.contains("deserialize_unit_struct"));
    assert!(unit.contains("Result::Ok(Unit)"));
}

#[test]
fn uses_unraw_identifiers_for_generated_names_and_wire_fields() {
    let input: DeriveInput = parse_quote! {
        struct r#Type {
            r#match: Sym,
        }
    };
    let output = compact(expand(&input).unwrap());
    assert!(output.contains("__InternityInternerForType"));
    assert!(output.contains("\"match\""));
    assert!(!output.contains("\"r#match\""));
}

#[test]
fn rejects_unsupported_input_shapes() {
    let generic: DeriveInput = parse_quote!(
        struct Generic<T>(T);
    );
    expand(&generic).expect_err("generic structs must be rejected");
    serialize(&generic).expect_err("generic structs must be rejected by SerializeIn");

    let enumeration: DeriveInput = parse_quote!(
        enum Choice {
            A,
        }
    );
    expand(&enumeration).expect_err("enums must be rejected");

    let union: DeriveInput = syn::parse2(quote!(union Choice { value: u32 })).unwrap();
    expand(&union).expect_err("unions must be rejected");
}

#[test]
fn rejects_conversion_container_attributes() {
    expand(&parse_quote!(
        #[serde(from = "Other")]
        struct T {
            value: Sym,
        }
    ))
    .expect_err("`from` cannot be honored by DeserializeIn");
    expand(&parse_quote!(
        #[serde(try_from = "Other")]
        struct T {
            value: Sym,
        }
    ))
    .expect_err("`try_from` cannot be honored by DeserializeIn");
    serialize(&parse_quote!(
        #[serde(into = "Other")]
        struct T {
            value: Sym,
        }
    ))
    .expect_err("`into` cannot be honored by SerializeIn");
}

#[test]
fn expansion_propagates_attribute_parse_errors() {
    expand(&parse_quote!(
        #[serde(rename = 5)]
        struct T {
            value: Sym,
        }
    ))
    .expect_err("deserialize expansion must propagate container attribute errors");
    serialize(&parse_quote!(
        #[serde(rename = 5)]
        struct T {
            value: Sym,
        }
    ))
    .expect_err("serialize expansion must propagate container attribute errors");
    expand(&parse_quote!(
        struct T {
            #[serde(rename = 5)]
            value: Sym,
        }
    ))
    .expect_err("named deserialize expansion must propagate field attribute errors");
    expand(&parse_quote!(
        struct T(#[serde(rename = 5)] Sym);
    ))
    .expect_err("tuple deserialize expansion must propagate field attribute errors");
    serialize(&parse_quote!(
        struct T {
            #[serde(rename = 5)]
            value: Sym,
        }
    ))
    .expect_err("named serialize expansion must propagate field attribute errors");
    serialize(&parse_quote!(
        struct T(#[serde(rename = 5)] Sym);
    ))
    .expect_err("tuple serialize expansion must propagate field attribute errors");
}

#[test]
fn serialize_rejects_skip_serializing_if() {
    // `DeserializeIn` still accepts the attribute, but `SerializeIn` refuses it
    // rather than silently ignoring the runtime skip predicate.
    expand(&parse_quote!(
        struct T {
            #[serde(skip_serializing_if = "Option::is_none")]
            value: Option<Sym>,
        }
    ))
    .expect("deserialize must accept skip_serializing_if");
    let named = serialize(&parse_quote!(
        struct T {
            #[serde(skip_serializing_if = "Option::is_none")]
            value: Option<Sym>,
        }
    ))
    .expect_err("named serialize must reject skip_serializing_if");
    assert!(named.to_string().contains("skip_serializing_if"));
    serialize(&parse_quote!(
        struct T(#[serde(skip_serializing_if = "Option::is_none")] Option<Sym>);
    ))
    .expect_err("tuple serialize must reject skip_serializing_if");

    // `skip_serializing` already drops the field, so pairing it with
    // `skip_serializing_if` is accepted (the predicate is irrelevant).
    serialize(&parse_quote!(
        struct T {
            #[serde(skip_serializing, skip_serializing_if = "Option::is_none")]
            value: Option<Sym>,
            keep: Sym,
        }
    ))
    .expect("skip_serializing exempts a field from the skip_serializing_if rejection");
}

#[test]
fn conflicting_modes_are_rejected_per_direction() {
    // `via_serde` + a deserialize-only `deserialize_with` is a valid *serialize*
    // schema, so only the deserialize expander rejects the contradiction.
    serialize(&parse_quote!(
        struct T {
            #[internity(via_serde)]
            #[serde(deserialize_with = "d")]
            value: Sym,
        }
    ))
    .expect("serialize must accept via_serde alongside a deserialize-only deserialize_with");
    let de = expand(&parse_quote!(
        struct T {
            #[internity(via_serde)]
            #[serde(deserialize_with = "d")]
            value: Sym,
        }
    ))
    .expect_err("deserialize must reject via_serde combined with deserialize_with");
    assert!(de.to_string().contains("mutually exclusive"));

    // Symmetrically, `via_serde` + a serialize-only `serialize_with` is a valid
    // *deserialize* schema, so only the serialize expander rejects it.
    expand(&parse_quote!(
        struct T {
            #[internity(via_serde)]
            #[serde(serialize_with = "s")]
            value: Sym,
        }
    ))
    .expect("deserialize must accept via_serde alongside a serialize-only serialize_with");
    let se = serialize(&parse_quote!(
        struct T {
            #[internity(via_serde)]
            #[serde(serialize_with = "s")]
            value: Sym,
        }
    ))
    .expect_err("serialize must reject via_serde combined with serialize_with");
    assert!(se.to_string().contains("mutually exclusive"));

    // `with` configures both directions, so both expanders reject it with `via_serde`.
    expand(&parse_quote!(
        struct T {
            #[internity(via_serde)]
            #[serde(with = "m")]
            value: Sym,
        }
    ))
    .expect_err("deserialize must reject via_serde combined with with");
    serialize(&parse_quote!(
        struct T {
            #[internity(via_serde)]
            #[serde(with = "m")]
            value: Sym,
        }
    ))
    .expect_err("serialize must reject via_serde combined with with");

    // Skipping deserialization while also requesting a custom deserializer is a
    // deserialize-side contradiction only.
    let skip = expand(&parse_quote!(
        struct T {
            #[internity(via_serde)]
            #[serde(skip_deserializing)]
            value: Sym,
        }
    ))
    .expect_err("deserialize must reject skip_deserializing combined with via_serde");
    assert!(skip.to_string().contains("custom deserializer mode"));
}

#[test]
fn with_seed_def_returns_none_without_custom_deserializer() {
    let root = super::append_module(&default_root(), parse_quote!(de));
    let name = parse_quote!(__Seed);
    let ty = parse_quote!(Sym);
    assert!(super::with_seed_def(&root, &name, &ty, &super::FieldAttrs::default()).is_none());
}

#[test]
fn transparent_validation_rejects_non_structs_when_called_directly() {
    let input: DeriveInput = parse_quote! {
        #[serde(transparent)]
        enum Choice { A }
    };
    let container = parse_container(&input.attrs).unwrap();
    super::validate_transparent_container(&input, &container).expect_err("transparent applies only to structs");
}

#[test]
fn supports_tuple_container_default_and_transparent_newtypes() {
    let defaulted = compact(
        expand(&parse_quote!(
            #[serde(default)]
            struct T(Sym);
        ))
        .unwrap(),
    );
    assert!(defaulted.contains("__container_default"));

    let transparent = compact(
        expand(&parse_quote!(
            #[serde(transparent)]
            struct T(Sym);
        ))
        .unwrap(),
    );
    assert!(transparent.contains("DeserializeSeed::deserialize"));

    expand(&parse_quote!(
        #[serde(default)]
        struct Unit;
    ))
    .expect_err("container default is invalid on unit structs");
    expand(&parse_quote!(
        #[serde(transparent)]
        struct Pair(Sym, Sym);
    ))
    .expect_err("transparent tuple structs must have one field");
    serialize(&parse_quote!(
        #[serde(transparent)]
        struct Pair(Sym, Sym);
    ))
    .expect_err("serialize transparent tuple structs must have one field");
}

#[test]
fn rejects_transparent_tuple_fields_that_cannot_be_targets() {
    expand(&parse_quote!(
        #[serde(transparent)]
        struct T(#[serde(skip)] u32);
    ))
    .expect_err("skipped transparent tuple field is not a target");
    expand(&parse_quote!(
        #[serde(transparent)]
        struct T(#[serde(default)] u32);
    ))
    .expect_err("defaulted transparent tuple field is not a deserialize target");
    expand(&parse_quote!(
        #[serde(transparent)]
        struct T(::core::marker::PhantomData<u32>);
    ))
    .expect_err("PhantomData transparent tuple field is not a target");
}

#[test]
fn expands_tuple_container_default_from_path() {
    let output = compact(
        expand(&parse_quote! {
            #[serde(default = "make_default")]
            struct T(Sym);
        })
        .unwrap(),
    );
    assert!(output.contains("make_default()"));
    assert!(output.contains("__container_default.0"));
}

#[test]
fn expands_container_default_named_structs() {
    // `Default::default()`-based container default: the present field with no
    // own default pulls from the container default binding.
    let trait_default = compact(
        expand(&parse_quote! {
            #[serde(default)]
            struct T { present: Sym, #[serde(default = "make")] own: u32 }
        })
        .unwrap(),
    );
    assert!(trait_default.contains("__container_default"));
    assert!(trait_default.contains("asDefault") || trait_default.contains("Default>::default"));

    // `default = "path"`-based container default uses the named constructor.
    let path_default = compact(
        expand(&parse_quote! {
            #[serde(default = "make_default")]
            struct T { present: Sym }
        })
        .unwrap(),
    );
    assert!(path_default.contains("make_default()"));
}

#[test]
fn uses_container_rename_in_deserializer_entry_points() {
    let named = compact(
        expand(&parse_quote! {
            #[serde(rename = "Wire")]
            struct T { value: Sym }
        })
        .unwrap(),
    );
    assert!(named.contains("deserialize_struct(\"Wire\""));

    let tuple = compact(
        expand(&parse_quote! {
            #[serde(rename = "WireTuple")]
            struct T(Sym);
        })
        .unwrap(),
    );
    assert!(tuple.contains("deserialize_newtype_struct(\"WireTuple\""));
}

#[test]
fn expands_skipped_field_defaults() {
    // A skipped field with its own `default = "path"` uses that constructor;
    // one without falls back to `Default::default()`.
    let output = compact(
        expand(&parse_quote! {
            struct T {
                kept: Sym,
                #[serde(skip, default = "seven")]
                skipped_path: u32,
                #[serde(skip)]
                skipped_trait: u32,
            }
        })
        .unwrap(),
    );
    assert!(output.contains("seven()"));
    assert!(output.contains("Default::default()"));
}

#[test]
fn expands_transparent_named_structs() {
    // Transparent: the single non-skipped field is deserialized directly and
    // the skipped companion is defaulted.
    let output = compact(
        expand(&parse_quote! {
            #[serde(transparent)]
            struct T {
                inner: Sym,
                #[serde(skip)]
                tag: u32,
            }
        })
        .unwrap(),
    );
    assert!(output.contains("DeserializeSeed::deserialize"));
    assert!(output.contains("Default::default()"));

    let defaulted = compact(
        expand(&parse_quote! {
            #[serde(transparent)]
            struct T {
                inner: Sym,
                #[serde(default)]
                tag: u32,
            }
        })
        .unwrap(),
    );
    assert!(defaulted.contains("letinner="));

    let phantom = compact(
        expand(&parse_quote! {
            #[serde(transparent)]
            struct T {
                inner: Sym,
                marker: ::core::marker::PhantomData<u32>,
            }
        })
        .unwrap(),
    );
    assert!(phantom.contains("letinner="));

    // More than one transparent field is rejected.
    expand(&parse_quote! {
        #[serde(transparent)]
        struct T { a: Sym, b: Sym }
    })
    .expect_err("transparent requires exactly one transparent field");
}

#[test]
fn expands_tuple_field_defaults() {
    // Tuple fields with defaults fill in on a short sequence; a skipped tuple
    // field is constructed from its default rather than the wire.
    let output = compact(
        expand(&parse_quote! {
            struct T(Sym, #[serde(default)] u32, #[serde(default = "seven")] u32, #[serde(skip)] u32);
        })
        .unwrap(),
    );
    assert!(output.contains("Default::default()"));
    assert!(output.contains("seven()"));
    assert!(output.contains("invalid_length"));
}

#[test]
fn rejects_required_tuple_field_after_default_without_container_default() {
    expand(&parse_quote! {
        struct T(#[serde(default)] u32, Sym);
    })
    .expect_err("serde rejects required tuple fields after defaulted fields");

    expand(&parse_quote! {
        #[serde(default)]
        struct T(#[serde(default)] u32, Sym);
    })
    .expect("container default makes later required tuple fields reachable");
}

#[test]
fn deserialize_tuple_builds_with_seed_defs() {
    let output = compact(
        expand(&parse_quote! {
            struct T(
                #[serde(deserialize_with = "my_de")]
                Sym,
                #[serde(with = "module")]
                Sym,
            );
        })
        .unwrap(),
    );
    assert!(output.contains("__InternityWithSeed0ForT"));
    assert!(output.contains("my_de(__d)"));
    assert!(output.contains("__InternityWithSeed1ForT"));
    assert!(output.contains("module::deserialize(__d)"));
}

#[test]
fn generated_helper_idents_are_collision_free_with_input_names() {
    // A custom `deserialize_with` function whose path collides with the
    // generated helper unit-struct name must not be shadowed: the helper is
    // renamed (trailing `_`) while the user's path is called unchanged.
    let output = compact(
        expand(&parse_quote! {
            struct T(#[serde(deserialize_with = "__InternityWithSeed0ForT")] Sym);
        })
        .unwrap(),
    );
    assert!(
        output.contains("struct__InternityWithSeed0ForT_;"),
        "generated seed struct should be renamed to avoid the collision: {output}"
    );
    assert!(
        output.contains("__InternityWithSeed0ForT(__d)"),
        "user's colliding function must still be invoked: {output}"
    );
}

#[test]
fn serialize_helper_idents_are_collision_free_with_input_names() {
    // Same guarantee on the serialization side for `serialize_with`.
    let output = compact(
        serialize(&parse_quote! {
            struct T(#[serde(serialize_with = "__InternitySerializeWith0ForT")] Sym);
        })
        .unwrap(),
    );
    assert!(
        output.contains("struct__InternitySerializeWith0ForT_"),
        "generated serialize adapter should be renamed to avoid the collision: {output}"
    );
    assert!(
        output.contains("__InternitySerializeWith0ForT(self.0,__serializer)"),
        "user's colliding function must still be invoked: {output}"
    );
}

#[test]
fn missing_value_expr_covers_every_fallback() {
    use super::{ContainerAttrs, DefaultValue, FieldAttrs, missing_value_expr};

    let ident: syn::Ident = parse_quote!(field);
    let binding = quote!(__container_default);
    let missing = quote!(return_missing);

    let no_container = ContainerAttrs::default();
    let di: DeriveInput = parse_quote!(
        #[serde(default)]
        struct S;
    );
    let with_container = parse_container(&di.attrs).unwrap();

    // Field with its own trait default.
    let own_trait = FieldAttrs {
        default: Some(DefaultValue::Trait),
        ..FieldAttrs::default()
    };
    assert!(compact(missing_value_expr(&ident, &own_trait, &no_container, &binding, &missing)).contains("Default::default()"));

    // Field with its own `default = "path"`.
    let own_path = FieldAttrs {
        default: Some(DefaultValue::Path(parse_quote!(make))),
        ..FieldAttrs::default()
    };
    assert!(compact(missing_value_expr(&ident, &own_path, &no_container, &binding, &missing)).contains("make()"));

    // No own default but a container default: pull from the container binding.
    let none = FieldAttrs::default();
    assert_eq!(
        compact(missing_value_expr(&ident, &none, &with_container, &binding, &missing)),
        "__container_default.field"
    );

    // No default anywhere: fall back to the hard-error token.
    assert_eq!(
        compact(missing_value_expr(&ident, &none, &no_container, &binding, &missing)),
        "return_missing"
    );
}

#[test]
fn codegen_pins_field_seed_and_with_definitions() {
    // A `deserialize_with` field emits its seed definition, whose body calls
    // the named function -- the only place that path token appears.
    let output = compact(
        expand(&parse_quote! {
            struct T {
                #[serde(deserialize_with = "myfn")]
                value: Sym,
            }
        })
        .unwrap(),
    );
    assert!(
        output.contains("myfn(__d)"),
        "with-seed body must invoke the named function: {output}"
    );
}

#[test]
fn codegen_container_default_binding_is_omitted_when_unneeded() {
    // Container default present, but every field either is skipped or carries
    // its own default: no field pulls from the container binding, so the
    // scratch binding must NOT be emitted.
    let output = compact(
        expand(&parse_quote! {
            #[serde(default)]
            struct T {
                #[serde(skip)]
                a: u32,
                #[serde(default = "mk")]
                b: Sym,
            }
        })
        .unwrap(),
    );
    assert!(
        !output.contains("__container_default"),
        "unneeded container default binding must be omitted: {output}"
    );
}

#[test]
fn codegen_transparent_targets_the_non_skipped_field() {
    let output = compact(
        expand(&parse_quote! {
            #[serde(transparent)]
            struct T {
                inner: Sym,
                #[serde(skip)]
                tag: u32,
            }
        })
        .unwrap(),
    );
    assert!(
        output.contains("letinner="),
        "transparent must deserialize the non-skipped field: {output}"
    );
}

#[test]
fn codegen_wire_surface_excludes_skipped_fields() {
    let output = compact(
        expand(&parse_quote! {
            struct T {
                kept: Sym,
                #[serde(skip)]
                hidden: u32,
            }
        })
        .unwrap(),
    );
    // The wire-name string only appears for non-skipped fields.
    assert!(output.contains("\"kept\""), "kept field must appear on the wire surface: {output}");
}

#[test]
fn codegen_wire_surface_includes_aliases() {
    let output = compact(
        expand(&parse_quote! {
            struct T {
                #[serde(alias = "also", alias = "legacy")]
                kept: Sym,
            }
        })
        .unwrap(),
    );
    assert!(output.contains("\"also\""));
    assert!(output.contains("\"legacy\""));
}

#[test]
fn deserialize_named_honors_rename_all_and_serialize_only_adapter_names() {
    let output = compact(
        expand(&parse_quote! {
            #[serde(rename_all = "camelCase")]
            struct T {
                #[serde(serialize_with = "my_ser")]
                first_field: Sym,
            }
        })
        .unwrap(),
    );
    assert!(output.contains("\"firstField\""));
}

#[test]
fn transparent_named_accepts_non_path_targets() {
    let output = compact(
        expand(&parse_quote! {
            #[serde(transparent)]
            struct T {
                inner: &'static Sym,
            }
        })
        .unwrap(),
    );
    assert!(output.contains("letinner="));
}

#[test]
fn codegen_ignore_variant_and_arm_track_deny_unknown_fields() {
    let lenient = compact(
        expand(&parse_quote! {
            struct T { a: Sym }
        })
        .unwrap(),
    );
    // The field enum carries the `__ignore` catch-all variant...
    assert!(
        lenient.contains("{__field0,__ignore,}"),
        "lenient field enum must declare __ignore: {lenient}"
    );
    // ...and the map visitor drains unknown values through IgnoredAny.
    assert!(
        lenient.contains("IgnoredAny"),
        "lenient map arm must ignore unknown values: {lenient}"
    );

    let strict = compact(
        expand(&parse_quote! {
            #[serde(deny_unknown_fields)]
            struct T { a: Sym }
        })
        .unwrap(),
    );
    assert!(
        !strict.contains("__ignore"),
        "deny_unknown_fields must not emit any __ignore handling: {strict}"
    );
}

#[test]
fn codegen_named_sequence_indices_increment() {
    let output = compact(
        expand(&parse_quote! {
            struct T { a: Sym, b: Sym }
        })
        .unwrap(),
    );
    // The second field's missing-in-sequence error reports index 1...
    assert!(
        output.contains("invalid_length(1,&self)"),
        "sequence index must advance to 1: {output}"
    );
    // ...and the trailing-element guard reports one past the arity (2 + 1).
    assert!(
        output.contains("invalid_length(3,&self)"),
        "trailing guard must report arity + 1: {output}"
    );
}

#[test]
fn codegen_tuple_sequence_indices_increment() {
    let output = compact(
        expand(&parse_quote! {
            struct T(Sym, Sym);
        })
        .unwrap(),
    );
    assert!(
        output.contains("invalid_length(1,&self)"),
        "tuple sequence index must advance to 1: {output}"
    );
    assert!(
        output.contains("invalid_length(3,&self)"),
        "tuple trailing guard must report arity + 1: {output}"
    );
}

#[test]
fn codegen_single_skipped_tuple_field_is_not_a_newtype() {
    let output = compact(
        expand(&parse_quote! {
            struct T(#[serde(skip)] u32);
        })
        .unwrap(),
    );
    assert!(
        !output.contains("visit_newtype_struct"),
        "a lone skipped field is not a newtype: {output}"
    );
}

#[test]
fn serializes_named_tuple_newtype_and_unit_structs() {
    let named = compact(
        serialize(&parse_quote! {
            struct Named {
                symbol: Sym,
                #[internity(via_serde)]
                count: u32,
            }
        })
        .unwrap(),
    );
    assert!(named.contains("serialize_struct"));
    assert!(named.contains("SerializeInWith::new(&self.symbol,__reader)"));
    assert!(named.contains("&&self.count"));

    let tuple = compact(
        serialize(&parse_quote! {
            struct Pair(Sym, u32);
        })
        .unwrap(),
    );
    assert!(tuple.contains("serialize_tuple_struct"));
    assert!(tuple.contains("SerializeTupleStruct::serialize_field"));

    let newtype = compact(
        serialize(&parse_quote! {
            struct Newtype(Sym);
        })
        .unwrap(),
    );
    assert!(newtype.contains("serialize_newtype_struct"));
    assert!(!newtype.contains("serialize_tuple_struct"));

    let unit = compact(
        serialize(&parse_quote! {
            struct Unit;
        })
        .unwrap(),
    );
    assert!(unit.contains("serialize_unit_struct"));
}

#[test]
fn serialize_honors_renames_and_skips() {
    let output = compact(
        serialize(&parse_quote! {
            #[serde(rename = "Wire", rename_all(serialize = "kebab-case", deserialize = "snake_case"))]
            struct T {
                first_field: Sym,
                #[serde(rename(serialize = "renamed", deserialize = "read_name"))]
                second_field: Sym,
                #[serde(skip_serializing)]
                hidden: Sym,
                #[serde(skip_deserializing)]
                write_only: Sym,
            }
        })
        .unwrap(),
    );
    assert!(output.contains("serialize_struct(__serializer,\"Wire\",3"));
    assert!(output.contains("\"first-field\""));
    assert!(output.contains("\"renamed\""));
    assert!(output.contains("\"write-only\""));
    assert!(!output.contains("\"hidden\""));
    assert!(!output.contains("\"read_name\""));
}

#[test]
fn serialize_named_honors_combined_rename_all() {
    let output = compact(
        serialize(&parse_quote! {
            #[serde(rename_all = "camelCase")]
            struct T {
                first_field: Sym,
            }
        })
        .unwrap(),
    );
    assert!(output.contains("\"firstField\""));
}

#[test]
fn serialize_transparent_delegates_to_the_target_field() {
    let output = compact(
        serialize(&parse_quote! {
            #[serde(transparent)]
            struct T {
                inner: Sym,
                marker: ::core::marker::PhantomData<u32>,
                #[serde(skip_serializing)]
                hidden: u32,
            }
        })
        .unwrap(),
    );
    assert!(output.contains("SerializeIn::serialize_in(&self.inner,__reader,__serializer)"));

    serialize(&parse_quote! {
        #[serde(transparent)]
        struct T {
            inner: Sym,
            #[serde(default)]
            extra: u32,
        }
    })
    .expect_err("serde Serialize rejects a defaulted non-skipped transparent companion");
}

#[test]
fn serialize_transparent_direct_call_modes_are_covered() {
    let via_serde = compact(
        serialize(&parse_quote! {
            #[serde(transparent)]
            struct T {
                #[internity(via_serde)]
                inner: Sym,
            }
        })
        .unwrap(),
    );
    assert!(via_serde.contains("Serialize::serialize(&self.inner,__serializer)"));

    let custom = compact(
        serialize(&parse_quote! {
            #[serde(transparent)]
            struct T {
                #[serde(serialize_with = "my_ser")]
                inner: Sym,
            }
        })
        .unwrap(),
    );
    assert!(custom.contains("my_ser(&self.inner,__serializer)"));

    let tuple = compact(
        serialize(&parse_quote! {
            #[serde(transparent)]
            struct T(Sym);
        })
        .unwrap(),
    );
    assert!(tuple.contains("SerializeIn::serialize_in(&self.0,__reader,__serializer)"));
}

#[test]
fn serialize_transparent_tuple_rejects_non_targets() {
    serialize(&parse_quote!(
        #[serde(transparent)]
        struct T(#[serde(skip_serializing)] u32);
    ))
    .expect_err("skip_serializing transparent tuple field is not a target");
    serialize(&parse_quote!(
        #[serde(transparent)]
        struct T(::core::marker::PhantomData<u32>);
    ))
    .expect_err("PhantomData transparent tuple field is not a target");
}

#[test]
fn serialize_uses_custom_serializer_adapters() {
    let output = compact(
        serialize(&parse_quote! {
            struct T {
                #[serde(serialize_with = "my_ser")]
                value: Sym,
                #[serde(with = "module")]
                other: Sym,
            }
        })
        .unwrap(),
    );
    assert!(output.contains("struct__InternitySerializeWith0ForT"));
    assert!(output.contains("my_ser(self.0,__serializer)"));
    assert!(output.contains("module::serialize(self.0,__serializer)"));
    assert!(output.contains("__InternitySerializeWith0ForT(&self.value)"));

    let tuple = compact(
        serialize(&parse_quote! {
            struct T(
                #[serde(serialize_with = "my_ser")]
                Sym,
                Sym,
            );
        })
        .unwrap(),
    );
    assert!(tuple.contains("__InternitySerializeWith0ForT"));
    assert!(tuple.contains("my_ser(self.0,__serializer)"));
    assert!(tuple.contains("__InternitySerializeWith0ForT(&self.0)"));

    let tuple_with = compact(
        serialize(&parse_quote! {
            struct T(
                #[serde(with = "module")]
                Sym,
                Sym,
            );
        })
        .unwrap(),
    );
    assert!(tuple_with.contains("__InternitySerializeWith0ForT"));
    assert!(tuple_with.contains("module::serialize(self.0,__serializer)"));
}

#[test]
fn public_entry_points_return_compile_errors_for_bad_input() {
    let root = default_root();
    let bad: proc_macro2::TokenStream = quote!(
        enum E {
            A,
        }
    );
    let deserialize = compact(super::derive_deserialize_in(bad.clone(), &root));
    let serialize = compact(super::derive_serialize_in(bad, &root));
    assert!(deserialize.contains("compile_error!"));
    assert!(serialize.contains("compile_error!"));
}

#[test]
fn transparent_named_struct_fills_its_non_target_siblings() {
    // A transparent struct deserializes its one real field and fills every
    // *other* field from a default. The target must be excluded from that
    // "others" set (via `!core::ptr::eq`); dropping the `!` would fill the
    // target twice and never mention the sibling.
    let input: DeriveInput = parse_quote! {
        #[serde(transparent)]
        struct Wrapper {
            inner: Sym,
            #[serde(skip)]
            marker: u32,
        }
    };
    let output = compact(expand(&input).unwrap());
    assert!(output.contains("marker"), "the skipped sibling must be filled: {output}");
}

#[test]
fn tuple_field_default_does_not_bind_the_container_default() {
    // With a container `default`, only *required* fields (not skipped, no own
    // default) force the container-default binding. A field carrying its own
    // `default` is not required, so no `__container_default` is emitted.
    // Guards the inner `&&` of that required-field predicate.
    let input: DeriveInput = parse_quote! {
        #[serde(default)]
        struct T(#[serde(default = "make")] Sym);
    };
    let output = compact(expand(&input).unwrap());
    assert!(
        !output.contains("__container_default"),
        "a field-defaulted tuple must not bind the container default: {output}"
    );
}

#[test]
fn serialize_tuple_with_a_skipped_field_stays_a_tuple_struct() {
    // A two-field tuple with one `skip_serializing` field has `plans.len() ==
    // 2` but `field_count == 1`; it must serialize as a tuple struct, not
    // collapse to a newtype. Guards the `&&` in the newtype predicate.
    let input: DeriveInput = parse_quote! {
        struct Pair(Sym, #[serde(skip_serializing)] u32);
    };
    let output = compact(serialize(&input).unwrap());
    assert!(
        output.contains("serialize_tuple_struct"),
        "must serialize as a tuple struct: {output}"
    );
    assert!(
        !output.contains("serialize_newtype_struct"),
        "must not collapse to a newtype: {output}"
    );
}
