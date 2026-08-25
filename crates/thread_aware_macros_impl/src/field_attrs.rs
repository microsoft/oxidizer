// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::{Attribute, Expr};

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
