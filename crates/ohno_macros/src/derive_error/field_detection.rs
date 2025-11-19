// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{Data, DeriveInput, Fields, Result, Type, TypePath};

use crate::derive_error::types::ErrorFieldRef;
use crate::utils::bail;

const NO_ERROR_FIELD: &str = "No field marked with `#[error]` found and no OhnoCore field detected. Either mark a field with `#[error]` or include a field of type OhnoCore";
const MULTIPLE_ERROR_FIELDS: &str = "Multiple OhnoCore fields found. Please mark the desired field with `#[error]` to disambiguate";

/// Find the field marked with `#[error]` or auto-detect `OhnoCore` field
pub fn find_error_field(input: &DeriveInput) -> Result<ErrorFieldRef> {
    let Data::Struct(data_struct) = &input.data else {
        bail!("Error derive only supports structs");
    };

    match &data_struct.fields {
        Fields::Named(fields) => find_error_field_named(fields),
        Fields::Unnamed(fields) => find_error_field_unnamed(fields),
        Fields::Unit => bail!("Error derive does not support unit structs"),
    }
}

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

/// Check if a field has the `#[error]` attribute
fn has_error_attribute(field: &syn::Field) -> bool {
    field.attrs.iter().any(|attr| attr.path().is_ident("error"))
}

/// Check if a type is `OhnoCore` or a variant of it
pub fn is_inner_error_type(ty: &Type) -> bool {
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
