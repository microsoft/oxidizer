// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::parse_quote;

use super::*;

fn parse_ok(input: DeriveInput) -> Ast {
    let mut errors = Errors::default();
    let ast = parse(input, &mut errors).expect("the input parses");
    assert!(errors.is_empty(), "unexpected faults: {}", errors.into_compile_error());
    ast
}

fn parse_faults(input: DeriveInput) -> String {
    let mut errors = Errors::default();
    _ = parse(input, &mut errors);
    errors.into_compile_error().to_string()
}

#[test]
fn reads_a_named_struct() {
    let ast = parse_ok(parse_quote! {
        struct T {
            path: String,
            #[error]
            inner: ohno::OhnoCore,
        }
    });

    assert_eq!(ast.style, Style::Named);
    assert_eq!(ast.fields.len(), 2);
    assert_eq!(member_name(&ast.fields[0].member), "path");
    assert!(ast.fields[0].marks.is_empty());
    assert_eq!(ast.fields[1].marks.len(), 1);
    assert!(ast.fields.iter().all(AstField::is_referenceable));
}

#[test]
fn reads_a_tuple_struct_by_index() {
    let ast = parse_ok(parse_quote!(
        struct T(String, #[error] ohno::OhnoCore);
    ));

    assert_eq!(ast.style, Style::Tuple);
    assert_eq!(member_name(&ast.fields[0].member), "0");
    assert_eq!(member_name(&ast.fields[1].member), "1");
}

#[test]
fn recognizes_the_generated_marker() {
    let ast = parse_ok(parse_quote! {
        struct T {
            path: String,
            #[doc = " ohno::generated-core@7f3d9c2a"]
            ohno_core: ohno::OhnoCore,
        }
    });

    assert!(ast.fields[0].is_referenceable());
    assert!(!ast.fields[1].is_referenceable());
}

#[test]
fn reads_the_display_attribute() {
    let ast = parse_ok(parse_quote! {
        #[display("failed for {path}: {}", code.value())]
        struct T {
            path: String,
            code: u32,
            inner: ohno::OhnoCore,
        }
    });

    let display = ast.display.expect("the display attribute is read");
    assert_eq!(display.template.value(), "failed for {path}: {}");
    assert_eq!(display.arguments.len(), 1);
}

#[test]
fn reads_the_flags() {
    let ast = parse_ok(parse_quote! {
        #[no_debug]
        #[no_constructors]
        struct T {
            inner: ohno::OhnoCore,
        }
    });

    assert!(ast.no_debug);
    assert!(ast.no_constructors);
}

#[test]
fn reads_several_from_attributes_and_their_overrides() {
    let ast = parse_ok(parse_quote! {
        #[from(std::io::Error(kind: error.kind()), std::fmt::Error)]
        #[from(std::num::ParseIntError)]
        struct T {
            kind: std::io::ErrorKind,
            inner: ohno::OhnoCore,
        }
    });

    assert_eq!(ast.conversions.len(), 3);
    assert_eq!(ast.conversions[0].overrides.len(), 1);
    assert_eq!(member_name(&ast.conversions[0].overrides[0].key), "kind");
    assert!(ast.conversions[1].overrides.is_empty());
}

#[test]
fn reads_a_tuple_keyed_from_override() {
    let ast = parse_ok(parse_quote! {
        #[from(std::io::Error(0: error.kind()))]
        struct T(std::io::ErrorKind, ohno::OhnoCore);
    });

    assert_eq!(member_name(&ast.conversions[0].overrides[0].key), "0");
}

#[test]
fn rejects_an_enum() {
    assert!(
        parse_faults(parse_quote!(
            enum T {
                A,
            }
        ))
        .contains("structs only")
    );
}

#[test]
fn rejects_a_union() {
    assert!(parse_faults(parse_quote!(union T { a: u32 })).contains("structs only"));
}

#[test]
fn rejects_a_unit_struct() {
    assert!(
        parse_faults(parse_quote!(
            struct T;
        ))
        .contains("unit struct has none")
    );
}

#[test]
fn rejects_a_marker_with_arguments_but_keeps_the_mark() {
    let mut errors = Errors::default();
    let ast = parse(
        parse_quote! {
            struct T {
                #[error(nonsense)]
                inner: Core,
            }
        },
        &mut errors,
    )
    .expect("the struct still parses");

    assert!(errors.into_compile_error().to_string().contains("takes no arguments"));
    assert_eq!(ast.fields[0].marks.len(), 1);
}

#[test]
fn rejects_every_malformed_marker_shape() {
    for input in [
        parse_quote! { struct T { #[error()] inner: Core, } },
        parse_quote! { struct T { #[error = "x"] inner: Core, } },
    ] {
        assert!(parse_faults(input).contains("takes no arguments"));
    }
}

#[test]
fn rejects_a_malformed_from_attribute() {
    for input in [
        parse_quote! { #[from] struct T { inner: ohno::OhnoCore, } },
        parse_quote! { #[from = "x"] struct T { inner: ohno::OhnoCore, } },
    ] {
        assert!(parse_faults(input).contains("parenthesized list of types"));
    }
}

#[test]
fn rejects_an_empty_from_attribute() {
    let faults = parse_faults(parse_quote! { #[from()] struct T { inner: ohno::OhnoCore, } });
    assert!(faults.contains("at least one type"), "{faults}");
}

#[test]
fn rejects_a_from_entry_that_names_no_type() {
    let faults = parse_faults(parse_quote! { #[from((kind: 1))] struct T { inner: ohno::OhnoCore, } });
    assert!(faults.contains("expected a type"), "{faults}");
}

#[test]
fn reads_a_source_type_written_with_parentheses() {
    // Parentheses appear inside types too, so a group is only read as an override list when it
    // holds `key: value` pairs.
    for input in [
        parse_quote! { #[from((u32, String))] struct T { inner: ohno::OhnoCore, } },
        parse_quote! { #[from(fn() -> std::io::Error)] struct T { inner: ohno::OhnoCore, } },
        parse_quote! { #[from(Box<dyn Fn() -> u8>)] struct T { inner: ohno::OhnoCore, } },
        parse_quote! { #[from(error_type!(kind: io))] struct T { inner: ohno::OhnoCore, } },
    ] {
        let mut errors = Errors::default();
        let ast = parse(input, &mut errors).expect("the input parses");
        assert!(errors.is_empty(), "{}", errors.into_compile_error());
        assert_eq!(ast.conversions.len(), 1);
    }
}

#[test]
fn reads_a_source_type_carrying_several_generic_arguments() {
    // A comma inside `<...>` separates generic arguments, not `#[from(...)]` entries.
    let ast = parse_ok(parse_quote! {
        #[from(PairError<u32, String>)]
        struct T { inner: ohno::OhnoCore, }
    });

    assert_eq!(ast.conversions.len(), 1);
}

#[test]
fn rejects_a_second_display_attribute() {
    let faults = parse_faults(parse_quote! {
        #[display("first")]
        #[display("second")]
        struct T { inner: ohno::OhnoCore, }
    });

    assert!(faults.contains("only one"), "{faults}");
}

#[test]
fn rejects_a_flag_that_carries_arguments() {
    for input in [
        parse_quote! { #[no_debug(foo)] struct T { inner: ohno::OhnoCore, } },
        parse_quote! { #[no_constructors = "x"] struct T { inner: ohno::OhnoCore, } },
    ] {
        assert!(parse_faults(input).contains("takes no arguments"));
    }
}

#[test]
fn rejects_a_malformed_display_attribute() {
    let faults = parse_faults(parse_quote! { #[display] struct T { inner: ohno::OhnoCore, } });
    assert!(!faults.is_empty());
}

#[test]
fn recognizes_an_ohno_core_type_by_its_last_segment() {
    assert!(is_ohno_core(&parse_quote!(OhnoCore)));
    assert!(is_ohno_core(&parse_quote!(ohno::OhnoCore)));
    assert!(!is_ohno_core(&parse_quote!(Core)));
    assert!(!is_ohno_core(&parse_quote!(&'a str)));
    assert!(!is_ohno_core(&parse_quote!(<T as Trait>::OhnoCore)));
}
