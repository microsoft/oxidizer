// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{Attribute, Expr, Type};

/// Configuration for field attributes.
#[derive(Default, Debug)]
pub struct FieldAttrCfg {
    /// Whether to skip this field in thread-aware processing.
    pub skip: bool,
}

/// Parses the `thread_aware` attributes on a field.
#[expect(clippy::missing_errors_doc, reason = "syn::internal API, no need for docs")]
pub fn parse_field_attrs(attrs: &[Attribute]) -> syn::Result<FieldAttrCfg> {
    let mut cfg = FieldAttrCfg::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("thread_aware")) {
        let parsed = attr.parse_args_with(|input: syn::parse::ParseStream| {
            if input.is_empty() {
                return Ok(None);
            }
            let expr: Expr = input.parse()?;
            Ok(Some(expr))
        })?;
        if let Some(expr) = parsed {
            match expr {
                Expr::Path(p) if p.path.is_ident("skip") => {
                    if cfg.skip {
                        return Err(syn::Error::new_spanned(p, "duplicate 'skip'"));
                    }
                    cfg.skip = true;
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "unknown thread_aware attribute (only 'skip' is supported)",
                    ));
                }
            }
        }
    }
    Ok(cfg)
}

/// Checks whether the given type is the standard library's `PhantomData` marker.
///
/// Only the canonical spellings are accepted: a bare `PhantomData` from a `use`, or a path
/// ending in `marker::PhantomData` rooted at `core`, `std` or nothing.
///
/// This decides bound selection only, never whether a field is relocated - every field
/// without the skip attribute is relocated whatever its type is called.
///
/// A spelling that matches gets a predicate on the field's own type,
/// `PhantomData<X>: ThreadAware`, rather than having the traversal descend into `X`. That
/// stays correct even for a look-alike imported under the bare name, since the predicate
/// names whatever type the spelling resolves to and that type's own impl decides what it
/// reduces to. A spelling that does not match, such as `my_crate::PhantomData<T>`, is
/// traversed normally: the derive descends into the arguments and binds the parameters it
/// reaches, which is the more general treatment.
#[must_use]
pub fn is_phantom_data(ty: &Type) -> bool {
    let Type::Path(tp) = ty else {
        return false;
    };

    let idents: Vec<_> = tp.path.segments.iter().map(|s| s.ident.to_string()).collect();
    let segments: Vec<&str> = idents.iter().map(String::as_str).collect();

    matches!(
        segments.as_slice(),
        ["PhantomData"] | ["marker", "PhantomData"] | ["core" | "std", "marker", "PhantomData"]
    )
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    #[test]
    fn test_parse_field_attrs_no_attrs() {
        // Test with no attributes at all
        let attrs: Vec<Attribute> = vec![];
        let result = parse_field_attrs(&attrs).unwrap();
        assert!(!result.skip);
    }

    #[test]
    fn test_parse_field_attrs_skip() {
        // Test with skip attribute
        let attrs: Vec<Attribute> = vec![parse_quote! { #[thread_aware(skip)] }];
        let result = parse_field_attrs(&attrs).unwrap();
        assert!(result.skip);
    }

    #[test]
    fn test_parse_field_attrs_empty_thread_aware() {
        // Test with empty thread_aware attribute
        let attrs: Vec<Attribute> = vec![parse_quote! { #[thread_aware()] }];
        let result = parse_field_attrs(&attrs).unwrap();
        assert!(!result.skip);
    }

    #[test]
    fn test_parse_field_attrs_duplicate_skip() {
        // Test that duplicate skip attributes are rejected
        let attrs: Vec<Attribute> = vec![parse_quote! { #[thread_aware(skip)] }, parse_quote! { #[thread_aware(skip)] }];
        let result = parse_field_attrs(&attrs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("duplicate 'skip'"));
    }

    #[test]
    fn test_parse_field_attrs_unknown_attribute() {
        // Test that unknown attributes are rejected
        let attrs: Vec<Attribute> = vec![parse_quote! { #[thread_aware(unknown)] }];
        let result = parse_field_attrs(&attrs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown thread_aware attribute"));
    }

    #[test]
    fn test_parse_field_attrs_unknown_attribute_with_value() {
        // Test that unknown attributes with values are rejected (covers line 30-33)
        let attrs: Vec<Attribute> = vec![parse_quote! { #[thread_aware(skip = helper)] }];
        let result = parse_field_attrs(&attrs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown thread_aware attribute"));
    }

    #[test]
    fn test_parse_field_attrs_non_thread_aware() {
        // Test that non-thread_aware attributes are ignored
        let attrs: Vec<Attribute> = vec![parse_quote! { #[derive(Debug)] }, parse_quote! { #[serde(skip)] }];
        let result = parse_field_attrs(&attrs).unwrap();
        assert!(!result.skip);
    }

    #[test]
    fn test_parse_field_attrs_mixed_attributes() {
        // Test that thread_aware attributes are parsed correctly alongside other attributes
        let attrs: Vec<Attribute> = vec![
            parse_quote! { #[derive(Debug)] },
            parse_quote! { #[thread_aware(skip)] },
            parse_quote! { #[serde(skip)] },
        ];
        let result = parse_field_attrs(&attrs).unwrap();
        assert!(result.skip);
    }

    #[test]
    fn test_is_phantom_data_simple() {
        // Test with simple PhantomData type
        let ty: Type = parse_quote! { PhantomData<T> };
        assert!(is_phantom_data(&ty));
    }

    #[test]
    fn test_is_phantom_data_with_std() {
        // Test with std::marker::PhantomData
        let ty: Type = parse_quote! { std::marker::PhantomData<T> };
        assert!(is_phantom_data(&ty));
    }

    #[test]
    fn test_is_phantom_data_with_core() {
        // Test with core::marker::PhantomData
        let ty: Type = parse_quote! { core::marker::PhantomData<T> };
        assert!(is_phantom_data(&ty));
    }

    #[test]
    fn test_is_phantom_data_fully_qualified() {
        // Test with fully qualified path
        let ty: Type = parse_quote! { ::std::marker::PhantomData<T> };
        assert!(is_phantom_data(&ty));
    }

    #[test]
    fn test_is_phantom_data_multiple_generics() {
        // Test with multiple generic parameters
        let ty: Type = parse_quote! { PhantomData<(T, U, V)> };
        assert!(is_phantom_data(&ty));
    }

    #[test]
    fn test_is_phantom_data_marker_module_only() {
        // `marker::PhantomData` after `use core::marker;`.
        let ty: Type = parse_quote! { marker::PhantomData<T> };
        assert!(is_phantom_data(&ty));
    }

    #[test]
    fn test_is_phantom_data_rejects_lookalike_paths() {
        // A distinct type that merely shares the final segment must not be taken for the
        // marker: it gets the more general treatment instead, with the traversal descending
        // into its arguments rather than a predicate on the type as a whole.
        let cases: Vec<Type> = vec![
            parse_quote! { my_crate::PhantomData<T> },
            parse_quote! { lookalike::inner::PhantomData<T> },
            parse_quote! { core::other::PhantomData<T> },
            parse_quote! { alloc::marker::PhantomData<T> },
        ];
        for ty in &cases {
            assert!(!is_phantom_data(ty), "a qualified look-alike must not be treated as the marker");
        }
    }

    #[test]
    fn test_is_phantom_data_not_phantom() {
        // Test with non-PhantomData types
        let ty: Type = parse_quote! { String };
        assert!(!is_phantom_data(&ty));

        let ty: Type = parse_quote! { Vec<u8> };
        assert!(!is_phantom_data(&ty));

        let ty: Type = parse_quote! { Option<T> };
        assert!(!is_phantom_data(&ty));
    }

    #[test]
    fn test_is_phantom_data_reference() {
        // Test with reference types (not a Type::Path)
        let ty: Type = parse_quote! { &PhantomData<T> };
        assert!(!is_phantom_data(&ty));
    }

    #[test]
    fn test_is_phantom_data_tuple() {
        // Test with tuple types (not a Type::Path)
        let ty: Type = parse_quote! { (PhantomData<T>,) };
        assert!(!is_phantom_data(&ty));
    }

    #[test]
    fn test_is_phantom_data_array() {
        // Test with array types (not a Type::Path)
        let ty: Type = parse_quote! { [PhantomData<T>; 1] };
        assert!(!is_phantom_data(&ty));
    }

    #[test]
    fn test_field_attr_cfg_default() {
        // Test that FieldAttrCfg::default() works correctly
        let cfg = FieldAttrCfg::default();
        assert!(!cfg.skip);
    }

    #[test]
    fn test_parse_field_attrs_covers_line_27() {
        // This test specifically covers line 27: if cfg.skip check
        // by attempting to set skip twice
        let attrs: Vec<Attribute> = vec![parse_quote! { #[thread_aware(skip)] }, parse_quote! { #[thread_aware(skip)] }];
        let result = parse_field_attrs(&attrs);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("duplicate"));
    }
}
