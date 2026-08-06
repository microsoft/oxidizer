// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{Data, DeriveInput, Fields, Meta, Result, Type, TypePath};

use crate::derive_error::types::ErrorFieldRef;
use crate::utils::{GENERATED_ERROR_FIELD_MARKER, bail, bail_spanned};

const NO_ERROR_FIELD: &str = "No field marked with `#[error]` found and no OhnoCore field detected. Either mark a field with `#[error]` or include a field of type OhnoCore";
const MULTIPLE_ERROR_FIELDS: &str = "Multiple OhnoCore fields found. Please mark the desired field with `#[error]` to disambiguate";
const ERROR_ATTRIBUTE_ARGUMENTS: &str = "`#[error]` takes no arguments";
const MULTIPLE_MARKED_FIELDS: &str = "Multiple fields marked with `#[error]`. Mark only the field holding the OhnoCore";
const DUPLICATE_MARKER: &str = "Duplicate `#[error]` on the same field. Mark it once";
const MARKED_FIELD_WITH_GENERATED: &str = "`#[ohno::error]` already added the field holding the OhnoCore and generates the error representation from it, so no field may be marked with `#[error]`. Remove the marker to keep the field as data, or use `#[derive(ohno::Error)]` on its own to place the OhnoCore yourself";

/// Validate every `#[error]` attribute in the struct
///
/// See `docs/error_error.md` for the rules this enforces.
pub(crate) fn validate_error_attributes(input: &DeriveInput) -> Result<()> {
    let Data::Struct(data_struct) = &input.data else {
        return Ok(());
    };

    let generated = data_struct.fields.iter().any(is_generated_error_field);

    let mut marked = 0;
    for field in &data_struct.fields {
        let mut markers = field.attrs.iter().filter(|attr| attr.path().is_ident("error"));

        let Some(attr) = markers.next() else {
            continue;
        };

        // A field carrying the marker twice says nothing a single one does not
        if let Some(duplicate) = markers.next() {
            bail_spanned!(duplicate, DUPLICATE_MARKER);
        }

        if !matches!(&attr.meta, Meta::Path(_)) {
            bail_spanned!(&attr.meta, ERROR_ATTRIBUTE_ARGUMENTS);
        }

        // The generated field is already the error representation, so a marker asks for a second
        // one. Reporting it here also keeps the two markers mutually exclusive, which is what lets
        // `has_error_attribute` treat either of them as decisive
        if generated {
            bail_spanned!(attr, MARKED_FIELD_WITH_GENERATED);
        }

        // Marking a second field leaves the choice of error field to declaration order, so it is
        // reported rather than resolved silently
        marked += 1;
        if marked > 1 {
            bail_spanned!(attr, MULTIPLE_MARKED_FIELDS);
        }
    }

    Ok(())
}

/// Find the field marked with `#[error]` or auto-detect `OhnoCore` field
pub(crate) fn find_error_field(input: &DeriveInput) -> Result<ErrorFieldRef> {
    let Data::Struct(data_struct) = &input.data else {
        bail!("Error derive only supports structs");
    };

    match &data_struct.fields {
        Fields::Named(fields) => find_error_field_named(fields),
        Fields::Unnamed(fields) => find_error_field_unnamed(fields),
        Fields::Unit => bail!("Error derive does not support unit structs"),
    }
}

#[expect(clippy::unwrap_used, reason = "Field names are guaranteed to be present here")]
fn find_error_field_named(fields: &syn::FieldsNamed) -> Result<ErrorFieldRef> {
    // First, look for fields explicitly marked with #[error]
    if let Some(field) = find_explicit_error_field_named(fields) {
        return Ok(ErrorFieldRef::Named(field));
    }

    // Auto-detect OhnoCore fields
    let fiasko_fields: Vec<_> = fields
        .named
        .iter()
        .filter(|&field| is_inner_error_type(&field.ty))
        .map(|field| field.ident.as_ref().unwrap())
        .collect();

    match fiasko_fields[..] {
        [] => bail!(NO_ERROR_FIELD),
        [field] => Ok(ErrorFieldRef::Named(field.clone())),
        _ => bail!(MULTIPLE_ERROR_FIELDS),
    }
}

fn find_error_field_unnamed(fields: &syn::FieldsUnnamed) -> Result<ErrorFieldRef> {
    // First, look for fields explicitly marked with #[error]
    if let Some(index) = find_explicit_error_field_unnamed(fields) {
        return Ok(ErrorFieldRef::Indexed(syn::Index::from(index)));
    }

    // Auto-detect OhnoCore fields
    let fiasko_indices: Vec<_> = fields
        .unnamed
        .iter()
        .enumerate()
        .filter(|(_, field)| is_inner_error_type(&field.ty))
        .map(|(index, _)| index)
        .collect();

    match fiasko_indices[..] {
        [] => bail!(NO_ERROR_FIELD),
        [index] => Ok(ErrorFieldRef::Indexed(syn::Index::from(index))),
        _ => bail!(MULTIPLE_ERROR_FIELDS),
    }
}

/// Find field explicitly marked with `#[error]` in named fields
fn find_explicit_error_field_named(fields: &syn::FieldsNamed) -> Option<syn::Ident> {
    fields
        .named
        .iter()
        .find(|field| has_error_attribute(field))
        .and_then(|field| field.ident.clone())
}

/// Find field explicitly marked with `#[error]` in unnamed fields
fn find_explicit_error_field_unnamed(fields: &syn::FieldsUnnamed) -> Option<usize> {
    fields
        .unnamed
        .iter()
        .enumerate()
        .find(|(_, field)| has_error_attribute(field))
        .map(|(index, _)| index)
}

/// Check if a field is the `OhnoCore` field injected by `#[ohno::error]`
pub(crate) fn is_generated_error_field(field: &syn::Field) -> bool {
    field.attrs.iter().any(|attr| match &attr.meta {
        Meta::NameValue(pair) if pair.path.is_ident("doc") => match &pair.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(text), ..
            }) => text.value() == GENERATED_ERROR_FIELD_MARKER,
            _ => false,
        },
        _ => false,
    })
}

/// Check if a field carries the marker naming it the one holding the `OhnoCore`
///
/// Either marker is decisive and the first match wins, which is sound only because
/// `validate_error_attributes` rejects a struct carrying both.
fn has_error_attribute(field: &syn::Field) -> bool {
    field.attrs.iter().any(|attr| attr.path().is_ident("error")) || is_generated_error_field(field)
}

/// Check if a type is `OhnoCore` or a variant of it
pub(crate) fn is_inner_error_type(ty: &Type) -> bool {
    let Type::Path(TypePath { path, .. }) = ty else {
        return false;
    };

    path.segments.last().is_some_and(|segment| segment.ident == "OhnoCore")
}

#[cfg(test)]
mod tests {
    use quote::ToTokens;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn test_find_error_field() {
        let input: DeriveInput = parse_quote! {
            struct TestError {
                message: String,
                #[error]
                inner: OhnoCore,
            }
        };

        let field = find_error_field(&input).unwrap();
        assert_eq!(field.to_string(), "inner");
    }

    #[test]
    fn test_auto_detect_inner_error_field() {
        let input: DeriveInput = parse_quote! {
            struct TestError {
                message: String,
                inner: OhnoCore,
            }
        };

        let field = find_error_field(&input).unwrap();
        assert_eq!(field.to_string(), "inner");
    }

    #[test]
    fn test_auto_detect_qualified_inner_error_field() {
        let input: DeriveInput = parse_quote! {
            struct TestError {
                message: String,
                error: ohno::OhnoCore,
            }
        };

        let field = find_error_field(&input).unwrap();
        assert_eq!(field.to_string(), "error");
    }

    #[test]
    fn test_explicit_error_attribute_takes_precedence() {
        let input: DeriveInput = parse_quote! {
            struct TestError {
                inner1: OhnoCore,
                #[error]
                inner2: OhnoCore,
            }
        };

        let field = find_error_field(&input).unwrap();
        assert_eq!(field.to_string(), "inner2");
    }

    #[test]
    fn test_multiple_inner_error_fields_require_explicit_attribute() {
        let input: DeriveInput = parse_quote! {
            struct TestError {
                inner1: OhnoCore,
                inner2: OhnoCore,
            }
        };

        let result = find_error_field(&input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Multiple OhnoCore fields found"));
    }

    #[test]
    fn test_no_error_fields_found() {
        let input: DeriveInput = parse_quote! {
            struct TestError {
                message: String,
                code: i32,
            }
        };

        let result = find_error_field(&input);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No field marked with `#[error]` found and no OhnoCore field detected")
        );
    }

    #[test]
    fn test_error_attribute_accepts_the_bare_marker() {
        // `#[error]` takes no arguments; the field `#[ohno::error]` injects carries a reserved
        // doc string instead, which is not part of this grammar
        let named: DeriveInput = parse_quote! {
            struct TestError { path: String, #[error] inner: OhnoCore }
        };
        validate_error_attributes(&named).unwrap();

        let generated: DeriveInput = parse_quote! {
            struct TestError { path: String, #[doc = " ohno::generated-core@7f3d9c2a"] ohno_core: OhnoCore }
        };
        validate_error_attributes(&generated).unwrap();

        let tuple: DeriveInput = parse_quote! { struct TestError(String, #[doc = " ohno::generated-core@7f3d9c2a"] OhnoCore); };
        validate_error_attributes(&tuple).unwrap();

        // Nothing to validate on an input that cannot carry the attribute
        let enum_input: DeriveInput = parse_quote! { enum TestError { A, B } };
        validate_error_attributes(&enum_input).unwrap();
    }

    #[test]
    fn test_error_attribute_rejects_any_argument() {
        // `#[error]` takes no arguments at all, including the one the macro used to write for
        // itself, so a user who copies `#[error(generated)]` out of an expansion is told so
        for input in [
            parse_quote! { struct TestError { #[error(generated)] inner: OhnoCore } },
            parse_quote! { struct TestError { #[error(generated, extra)] inner: OhnoCore } },
            parse_quote! { struct TestError { #[error(generated = true)] inner: OhnoCore } },
            parse_quote! { struct TestError { #[error("generated")] inner: OhnoCore } },
            parse_quote! { struct TestError { #[error()] inner: OhnoCore } },
            parse_quote! { struct TestError { #[error = "x"] inner: OhnoCore } },
            parse_quote! { struct TestError(String, #[error(nonsense)] OhnoCore); },
        ] {
            let input: DeriveInput = input;
            let message = validate_error_attributes(&input).unwrap_err().to_string();
            assert_eq!(message, ERROR_ATTRIBUTE_ARGUMENTS);
        }
    }

    #[test]
    fn test_error_attribute_rejects_a_second_marked_field() {
        // Which field wins would otherwise come down to declaration order
        for input in [
            parse_quote! { struct TestError { #[error] first: OhnoCore, #[error] second: OhnoCore } },
            parse_quote! { struct TestError(#[error] OhnoCore, #[error] OhnoCore); },
            parse_quote! { struct TestError { #[error] inner: OhnoCore, #[error] ohno_core: OhnoCore } },
        ] {
            let input: DeriveInput = input;
            let message = validate_error_attributes(&input).unwrap_err().to_string();
            assert_eq!(message, MULTIPLE_MARKED_FIELDS);
        }
    }

    #[test]
    fn test_error_attribute_rejects_a_marker_beside_the_generated_field() {
        // The generated field is already the error representation, so a marker asks for a second
        // one. This is also what keeps the two markers mutually exclusive, so that treating either
        // as decisive cannot resolve the choice by declaration order
        let marker = GENERATED_ERROR_FIELD_MARKER;
        for input in [
            parse_quote! { struct TestError { #[error] inner: OhnoCore, #[doc = #marker] ohno_core: OhnoCore } },
            parse_quote! { struct TestError { #[doc = #marker] ohno_core: OhnoCore, #[error] inner: OhnoCore } },
            parse_quote! { struct TestError(#[error] OhnoCore, #[doc = #marker] OhnoCore); },
            parse_quote! { struct TestError { #[error] not_a_core: String, #[doc = #marker] ohno_core: OhnoCore } },
        ] {
            let input: DeriveInput = input;
            let message = validate_error_attributes(&input).unwrap_err().to_string();
            assert_eq!(message, MARKED_FIELD_WITH_GENERATED);
        }
    }

    #[test]
    fn test_error_attribute_rejects_a_duplicate_marker_on_one_field() {
        // One field marked twice is one field, so it is reported as the duplicate it is rather
        // than as a second marked field
        for input in [
            parse_quote! { struct TestError { #[error] #[error] inner: OhnoCore } },
            parse_quote! { struct TestError(#[error] #[error] OhnoCore); },
        ] {
            let input: DeriveInput = input;
            let message = validate_error_attributes(&input).unwrap_err().to_string();
            assert_eq!(message, DUPLICATE_MARKER);
        }
    }

    #[test]
    fn test_marked_field_type_is_left_to_the_generated_implementations() {
        // The marker designates a field; a core reached through an alias or a rename is spelled
        // however the user spelled it, which only `rustc` can resolve
        for input in [
            parse_quote! { struct TestError { #[error] aliased: MyCore } },
            parse_quote! { struct TestError(String, #[error] MyCore); },
            parse_quote! { struct TestError { #[error] inner: ohno::OhnoCore } },
            parse_quote! { struct TestError { #[error] inner: crate::OhnoCore } },
            parse_quote! { struct TestError(String, #[doc = " ohno::generated-core@7f3d9c2a"] ohno::OhnoCore); },
        ] {
            let input: DeriveInput = input;
            validate_error_attributes(&input).unwrap();
        }
    }

    #[test]
    fn test_injected_field_is_recognized_by_its_doc_marker() {
        // The whole string is the contract. A doc string can only fail to match, never be
        // rejected, so anything short of the exact marker has to be somebody's own doc comment
        let marker = GENERATED_ERROR_FIELD_MARKER;
        let generated: syn::Field = parse_quote! { #[doc = #marker] ohno_core: OhnoCore };
        assert!(is_generated_error_field(&generated));

        for field in [
            // Doc comments of the user's own, including ones that name the crate
            parse_quote! { #[doc = " an ordinary doc comment"] inner: OhnoCore },
            parse_quote! { #[doc = " the ohno generated core field"] inner: OhnoCore },
            parse_quote! { #[doc = " ohno::generated-core"] inner: OhnoCore },
            parse_quote! { #[doc = "ohno::generated-core@7f3d9c2a"] inner: OhnoCore },
            parse_quote! { #[doc = " ohno::generated-core@7f3d9c2a "] inner: OhnoCore },
            parse_quote! { #[doc = " ohno::generated-core@7f3d9c2b"] inner: OhnoCore },
            // The marker under another attribute, or as a non-string value, says nothing
            parse_quote! { #[cfg_attr(doc, doc = " ohno::generated-core@7f3d9c2a")] inner: OhnoCore },
            parse_quote! { #[error(generated)] inner: OhnoCore },
            parse_quote! { #[error] inner: OhnoCore },
            parse_quote! { inner: OhnoCore },
        ] {
            let field: syn::Field = field;
            assert!(!is_generated_error_field(&field), "matched: {}", field.to_token_stream());
        }
    }

    #[test]
    fn test_marker_is_not_plausible_prose() {
        // A marker that read as an ordinary doc comment would let a user hide their own field by
        // documenting it, and nothing can reject that, so the nonce is what keeps it out of reach
        assert!(
            GENERATED_ERROR_FIELD_MARKER.contains("7f3d9c2a"),
            "the marker must carry a nonce no doc comment would contain"
        );
    }

    #[test]
    fn test_injected_field_is_found_without_a_marker_attribute() {
        // The doc marker replaces the `#[error]` the attribute used to write, so field lookup has
        // to recognise it as well, or `#[ohno::error]` structs lose their error field
        let marker = GENERATED_ERROR_FIELD_MARKER;

        let named: DeriveInput = parse_quote! {
            struct TestError { path: String, #[doc = #marker] ohno_core: ohno::OhnoCore }
        };
        assert_eq!(find_error_field(&named).unwrap().to_string(), "ohno_core");

        let tuple: DeriveInput = parse_quote! {
            struct TestError(String, #[doc = #marker] ohno::OhnoCore);
        };
        assert_eq!(find_error_field(&tuple).unwrap().to_string(), "1");
    }

    #[test]
    fn test_find_error_field_in_tuple() {
        let input: DeriveInput = parse_quote! { struct TestError( String, #[error] OhnoCore); };
        let field = find_error_field(&input).unwrap();
        assert_eq!(field.to_string(), "1");
    }

    #[test]
    fn test_find_unmarked_error_field_in_tuple() {
        let input: DeriveInput = parse_quote! { struct TestError( String, OhnoCore); };
        let field = find_error_field(&input).unwrap();
        assert_eq!(field.to_string(), "1");
    }

    #[test]
    fn test_find_missing_error_field_in_tuple() {
        let input: DeriveInput = parse_quote! { struct TestError( String, String); };
        let err = find_error_field(&input).unwrap_err();
        assert!(err.to_string().contains(NO_ERROR_FIELD));
    }

    #[test]
    fn test_double_field_in_tuple() {
        let input: DeriveInput = parse_quote! { struct TestError( String, OhnoCore, OhnoCore); };
        let err = find_error_field(&input).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Multiple OhnoCore fields found. Please mark the desired field with `#[error]` to disambiguate"
        );
    }

    #[test]
    fn test_marked_field_with_another_type_in_tuple() {
        // The lookup answers only which field is marked; whether that field may hold the type it
        // does is settled beforehand by `validate_error_attributes`
        let input: DeriveInput = parse_quote! { struct TestError( String, #[error] MyCore); };
        let field = find_error_field(&input).unwrap();
        assert_eq!(field.to_string(), "1");
    }

    #[test]
    fn test_is_inner_error_type() {
        let simple_inner_error: Type = syn::parse_str("OhnoCore").unwrap();
        let qualified_inner_error: Type = syn::parse_str("ohno::OhnoCore").unwrap();
        let crate_inner_error: Type = syn::parse_str("crate::OhnoCore").unwrap();
        let other_type: Type = syn::parse_str("String").unwrap();
        let other_error_type: Type = syn::parse_str("MyError").unwrap();

        assert!(is_inner_error_type(&simple_inner_error));
        assert!(is_inner_error_type(&qualified_inner_error));
        assert!(is_inner_error_type(&crate_inner_error));
        assert!(!is_inner_error_type(&other_type));
        assert!(!is_inner_error_type(&other_error_type));
    }

    #[test]
    fn test_is_inner_error_type_non_path() {
        let reference_inner_error: Type = syn::parse_str("&OhnoCore").unwrap();

        assert!(!is_inner_error_type(&reference_inner_error));
    }

    #[test]
    fn test_find_error_field_rejects_non_structs() {
        let input: DeriveInput = parse_quote! {
            enum TestError { Variant(OhnoCore) }
        };

        let err = find_error_field(&input).unwrap_err();
        assert_eq!(err.to_string(), "Error derive only supports structs");
    }

    #[test]
    fn test_find_error_field_rejects_unit_structs() {
        let input: DeriveInput = parse_quote! {
            struct TestError;
        };

        let err = find_error_field(&input).unwrap_err();
        assert_eq!(err.to_string(), "Error derive does not support unit structs");
    }

    #[test]
    fn test_find_explicit_error_field_unnamed() {
        let fields: syn::FieldsUnnamed = parse_quote! { (String, #[error] OhnoCore, OhnoCore) };

        let index = find_explicit_error_field_unnamed(&fields).expect("should find error attribute");
        assert_eq!(index, 1);
    }
}
