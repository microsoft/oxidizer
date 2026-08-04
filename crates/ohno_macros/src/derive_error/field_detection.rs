// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{Data, DeriveInput, Fields, Meta, Result, Type, TypePath};

use crate::derive_error::types::ErrorFieldRef;
use crate::utils::{bail, bail_spanned, generated_error_field_marker};

const NO_ERROR_FIELD: &str = "No field marked with `#[error]` found and no OhnoCore field detected. Either mark a field with `#[error]` or include a field of type OhnoCore";
const MULTIPLE_ERROR_FIELDS: &str = "Multiple OhnoCore fields found. Please mark the desired field with `#[error]` to disambiguate";
const ERROR_ATTRIBUTE_ARGUMENTS: &str = "`#[error]` takes no arguments";
const MULTIPLE_MARKED_FIELDS: &str = "Multiple fields marked with `#[error]`. Mark only the field holding the OhnoCore";
const DUPLICATE_MARKER: &str = "Duplicate `#[error]` on the same field. Mark it once";
const MARKED_FIELD_TYPE: &str = "`#[error]` marks the field holding the OhnoCore, so it cannot appear on a field of another type. Refer to the type by its own name if it is reached through an alias or a rename";

/// Validate every `#[error]` attribute in the struct
///
/// The attribute marks the field holding the `OhnoCore` and takes no arguments. Its sole argument
/// form, `#[error(generated)]`, is written by `#[ohno::error]` onto the field it injects, and
/// tells the rest of the macro that the field is not part of the surface the user wrote.
///
/// Both the argument list and the marked field's type are checked here, where the mistake is, so
/// that a marker on the wrong field is reported against the field rather than against the
/// implementations generated from it.
pub(crate) fn validate_error_attributes(input: &DeriveInput) -> Result<()> {
    let Data::Struct(data_struct) = &input.data else {
        return Ok(());
    };

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

        match &attr.meta {
            Meta::Path(_) => {}
            Meta::List(list) if is_generated_marker(list) => {}
            other => bail_spanned!(other, ERROR_ATTRIBUTE_ARGUMENTS),
        }

        if !is_inner_error_type(&field.ty) {
            bail_spanned!(&field.ty, MARKED_FIELD_TYPE);
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

/// Check whether an `#[error(...)]` argument list is the generated-field marker
fn is_generated_marker(list: &syn::MetaList) -> bool {
    syn::parse2::<syn::Ident>(list.tokens.clone()).is_ok_and(|marker| marker == generated_error_field_marker())
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
        Meta::List(list) => list.path.is_ident("error") && is_generated_marker(list),
        _ => false,
    })
}

/// Check if a field has the `#[error]` attribute
fn has_error_attribute(field: &syn::Field) -> bool {
    field.attrs.iter().any(|attr| attr.path().is_ident("error"))
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
    fn test_error_attribute_accepts_its_two_forms() {
        // The bare marker the user writes, and the one `#[ohno::error]` writes onto the field it
        // injects
        let named: DeriveInput = parse_quote! {
            struct TestError { path: String, #[error] inner: OhnoCore }
        };
        validate_error_attributes(&named).unwrap();

        let generated: DeriveInput = parse_quote! {
            struct TestError { path: String, #[error(generated)] ohno_core: OhnoCore }
        };
        validate_error_attributes(&generated).unwrap();

        let tuple: DeriveInput = parse_quote! { struct TestError(String, #[error(generated)] OhnoCore); };
        validate_error_attributes(&tuple).unwrap();

        // Nothing to validate on an input that cannot carry the attribute
        let enum_input: DeriveInput = parse_quote! { enum TestError { A, B } };
        validate_error_attributes(&enum_input).unwrap();
    }

    #[test]
    fn test_error_attribute_rejects_unrecognized_arguments() {
        // A marker that does not say what it means must not pass for one that does: an argument
        // list resembling the generated marker would otherwise be silently ignored, leaving the
        // injected field indistinguishable from a field the user declared
        for input in [
            parse_quote! { struct TestError { #[error(generatd)] inner: OhnoCore } },
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
            parse_quote! { struct TestError { #[error] inner: OhnoCore, #[error(generated)] ohno_core: OhnoCore } },
        ] {
            let input: DeriveInput = input;
            let message = validate_error_attributes(&input).unwrap_err().to_string();
            assert_eq!(message, MULTIPLE_MARKED_FIELDS);
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
    fn test_marked_field_must_hold_the_core_type() {
        // The marker says the field holds the OhnoCore, so a field of another type is reported
        // here rather than by the implementations generated from it
        for input in [
            parse_quote! { struct TestError { #[error] not_a_core: String } },
            parse_quote! { struct TestError(#[error] String); },
            parse_quote! { struct TestError { #[error(generated)] hidden: String, inner: OhnoCore } },
            parse_quote! { struct TestError { #[error] aliased: MyCore } },
        ] {
            let input: DeriveInput = input;
            let message = validate_error_attributes(&input).unwrap_err().to_string();
            assert_eq!(message, MARKED_FIELD_TYPE);
        }

        // The type may be reached through a path, which is how `#[ohno::error]` writes it
        for input in [
            parse_quote! { struct TestError { #[error] inner: ohno::OhnoCore } },
            parse_quote! { struct TestError { #[error] inner: crate::OhnoCore } },
            parse_quote! { struct TestError(String, #[error(generated)] ohno::OhnoCore); },
        ] {
            let input: DeriveInput = input;
            validate_error_attributes(&input).unwrap();
        }
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
