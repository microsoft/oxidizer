// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

#[test]
fn gamma_round2_concrete_path_segments_are_not_generic_parameters() {
    let mut usage = GenericUsage::default();
    let params = std::iter::once(parse_quote!(T)).collect();
    collect_generics_in_type(&parse_quote!(::std::vec::Vec<(T, u32)>), &params, &mut usage).unwrap();
    assert_eq!(usage.relocated, std::iter::once(parse_quote!(T)).collect());
}

#[test]
fn gamma_round2_body_parsers_propagate_field_errors() {
    let root = parse_quote!(::thread_aware);
    let name = parse_quote!(Example);
    for input in [
        parse_quote!(
            struct Example {
                #[thread_aware(unknown)]
                value: u32,
            }
        ),
        parse_quote!(
            struct Example(#[thread_aware(unknown)] u32);
        ),
    ] {
        let input: DeriveInput = input;
        let Data::Struct(data) = input.data else {
            panic!("expected struct")
        };
        assert_eq!(
            build_struct_body(&name, &data.fields, &root).unwrap_err().to_string(),
            "unknown thread_aware attribute (only 'skip' is supported)"
        );
    }
    for input in [
        parse_quote!(
            enum Example {
                Named {
                    #[thread_aware(unknown)]
                    value: u32,
                },
            }
        ),
        parse_quote!(
            enum Example {
                Tuple(#[thread_aware(unknown)] u32),
            }
        ),
    ] {
        let input: DeriveInput = input;
        let Data::Enum(data) = input.data else { panic!("expected enum") };
        assert_eq!(
            build_enum_body(&name, &data, &root).unwrap_err().to_string(),
            "unknown thread_aware attribute (only 'skip' is supported)"
        );
    }
}
