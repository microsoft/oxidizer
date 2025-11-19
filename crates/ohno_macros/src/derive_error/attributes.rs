// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::spanned::Spanned;
use syn::{DeriveInput, Expr, Ident, Lit, Meta, Result, Type};

use crate::derive_error::types::FromConfig;
use crate::utils::bail;

const FROM_EMPTY_PARENS: &str =
    "empty #[from()] attribute is not allowed. Either specify types like #[from(ErrorType)] or remove the attribute entirely";
const FROM_EMPTY_PATH: &str =
    "empty #[from] attribute is not allowed. Either specify types like #[from(ErrorType)] or remove the attribute entirely";
const FROM_INVALID_FORM: &str = "from attribute must be in the form #[from(Type1, Type2, ...)] with at least one type specified";

/// Structure to hold display attribute information
#[derive(Debug)]
pub struct DisplayAttribute {
    pub template: String,
    pub args: Vec<syn::Expr>,
}

/// Find the display attribute value
pub fn find_display_attribute(input: &DeriveInput) -> Result<Option<DisplayAttribute>> {
    for attr in &input.attrs {
        if attr.path().is_ident("display") {
            let Meta::List(meta_list) = &attr.meta else {
                bail!(
                    attr.span(),
                    "display attribute must be in the form #[display(\"message\")] or #[display(\"format_template\", arg1, arg2)]"
                );
            };

            return parse_display_tokens(&meta_list.tokens).map(Some);
        }
    }
    Ok(None)
}

fn parse_display_tokens(tokens: &proc_macro2::TokenStream) -> Result<DisplayAttribute> {
    syn::parse::Parser::parse2(
        |input: syn::parse::ParseStream| {
            // Parse the format string first
            let template_lit: Lit = input.parse()?;
            let Lit::Str(template_str) = template_lit else {
                bail!(template_lit.span(), "display attribute template must be a string literal");
            };

            let mut args = Vec::new();

            // Parse optional arguments
            while input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
                if !input.is_empty() {
                    let expr: Expr = input.parse()?;
                    args.push(expr);
                }
            }

            Ok(DisplayAttribute {
                template: template_str.value(),
                args,
            })
        },
        tokens.clone(),
    )
}

/// Check if the `no_constructors` attribute is present
pub fn has_no_constructors_attribute(input: &DeriveInput) -> bool {
    has_simple_attribute(input, "no_constructors")
}

/// Check if the `no_debug` attribute is present
pub fn has_no_debug_attribute(input: &DeriveInput) -> bool {
    has_simple_attribute(input, "no_debug")
}

/// Parse the from attribute to get types for From trait implementation
pub fn find_from_attribute(input: &DeriveInput) -> Result<Vec<FromConfig>> {
    let mut from_configs = Vec::new();
    for attr in &input.attrs {
        if attr.path().is_ident("from") {
            let mut configs = parse_from_configs_from_meta(&attr.meta, attr.span())?;
            from_configs.append(&mut configs);
        }
    }
    Ok(from_configs)
}

// Helper functions

/// Check if a simple attribute (no parameters) is present
fn has_simple_attribute(input: &DeriveInput, attr_name: &str) -> bool {
    input.attrs.iter().any(|attr| attr.path().is_ident(attr_name))
}

fn parse_from_configs_from_meta(meta: &Meta, span: proc_macro2::Span) -> Result<Vec<FromConfig>> {
    match meta {
        Meta::List(meta_list) => {
            if meta_list.tokens.is_empty() {
                bail!(span, FROM_EMPTY_PARENS)
            }
            parse_from_config_list(&meta_list.tokens)
        }
        Meta::Path(_) => bail!(span, FROM_EMPTY_PATH),
        Meta::NameValue(_) => bail!(span, FROM_INVALID_FORM),
    }
}

/// Parse a comma-separated list of from configurations with optional field expressions
fn parse_from_config_list(tokens: &proc_macro2::TokenStream) -> Result<Vec<FromConfig>> {
    use std::collections::HashMap;

    let mut configs = Vec::new();
    syn::parse::Parser::parse2(
        |input: syn::parse::ParseStream| {
            while !input.is_empty() {
                // Parse the type first
                let from_type: Type = input.parse()?;

                // Check if there are field expressions in parentheses
                let field_expressions = if input.peek(syn::token::Paren) {
                    let content;
                    syn::parenthesized!(content in input);
                    parse_field_expressions(&content)?
                } else {
                    HashMap::new()
                };

                configs.push(FromConfig {
                    from_type,
                    field_expressions,
                });

                if !input.is_empty() {
                    input.parse::<syn::Token![,]>()?;
                }
            }
            Ok(())
        },
        tokens.clone(),
    )?;
    Ok(configs)
}

/// Parse field expressions: field: value, field2: value2, 0: value3
fn parse_field_expressions(content: syn::parse::ParseStream) -> Result<std::collections::HashMap<String, Expr>> {
    let mut field_expressions = std::collections::HashMap::new();

    while !content.is_empty() {
        let field_key = parse_field_key(content)?;
        content.parse::<syn::Token![:]>()?;
        let expr: Expr = content.parse()?;
        field_expressions.insert(field_key, expr);

        if !content.is_empty() {
            content.parse::<syn::Token![,]>()?;
        }
    }

    Ok(field_expressions)
}

/// Parse field name - can be either an identifier or a numeric literal
fn parse_field_key(content: syn::parse::ParseStream) -> Result<String> {
    if content.peek(syn::Lit) {
        // Handle numeric field (for tuple structs): 0, 1, 2, etc.
        let lit: syn::Lit = content.parse()?;
        match lit {
            syn::Lit::Int(lit_int) => Ok(lit_int.base10_digits().to_string()),
            _ => Err(syn::Error::new(
                lit.span(),
                "Only integer literals are supported for tuple field indices",
            )),
        }
    } else {
        // Handle named field (for regular structs): field_name
        let field_name: Ident = content.parse()?;
        Ok(field_name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn expect_display_error(input: &DeriveInput, expected: &str) {
        let err = find_display_attribute(input).unwrap_err();
        assert!(
            err.to_string().contains(expected),
            "Expected error to contain '{expected}', got '{err}'"
        );
    }

    fn expect_from_error(input: &DeriveInput, expected: &str) {
        let err = find_from_attribute(input).unwrap_err();
        assert!(
            err.to_string().contains(expected),
            "Expected error to contain '{expected}', got '{err}'"
        );
    }

    fn assert_field_keys(config: &FromConfig, expected: &[&str]) {
        assert_eq!(config.field_expressions.len(), expected.len());
        for key in expected {
            assert!(config.field_expressions.contains_key(*key), "Missing expected key '{key}'");
        }
    }

    #[test]
    fn test_find_display_attribute() {
        let input: DeriveInput = parse_quote! {
            #[display("Failed to read config with path: {path}")]
            struct ConfigError {
                pub path: String,
                #[error]
                pub inner_error: OhnoCore,
            }
        };

        let attr = find_display_attribute(&input).unwrap();
        assert!(attr.is_some());
        let attr = attr.unwrap();
        assert_eq!(attr.template, "Failed to read config with path: {path}");
        assert!(attr.args.is_empty());
    }

    #[test]
    fn test_find_display_attribute_error_cases() {
        let cases = vec![
            (
                parse_quote! {
                    #[display]
                    struct ConfigError {
                        pub path: String,
                        #[error]
                        pub inner_error: OhnoCore,
                    }
                },
                "display attribute must be in the form",
            ),
            (
                parse_quote! {
                    #[display(1, 2, 3)]
                    struct ConfigError {
                        pub path: String,
                        #[error]
                        pub inner_error: OhnoCore,
                    }
                },
                "display attribute template must be a string literal",
            ),
        ];

        for (input, expected) in cases {
            expect_display_error(&input, expected);
        }
    }

    #[test]
    fn test_find_display_attribute_with_args() {
        let input: DeriveInput = parse_quote! {
            #[display("Invalid data: {} - {}", data.0, data.1, )]
            struct InvalidData {
                pub data: Data,
                #[error]
                pub inner_error: OhnoCore,
            }
        };

        let attr = find_display_attribute(&input).unwrap();
        assert!(attr.is_some());
        let attr = attr.unwrap();
        assert_eq!(attr.template, "Invalid data: {} - {}");
        assert_eq!(attr.args.len(), 2);
    }

    #[test]
    fn test_has_no_constructors_attribute() {
        let input_with: DeriveInput = parse_quote! {
            #[no_constructors]
            struct TestError {
                #[error]
                inner: OhnoCore,
            }
        };

        let input_without: DeriveInput = parse_quote! {
            struct TestError {
                #[error]
                inner: OhnoCore,
            }
        };

        assert!(has_no_constructors_attribute(&input_with));
        assert!(!has_no_constructors_attribute(&input_without));
    }

    #[test]
    fn test_find_from_attribute_valid() {
        let input: DeriveInput = parse_quote! {
            #[from(std::io::Error, std::fmt::Error)]
            struct TestError {
                #[error]
                inner: OhnoCore,
            }
        };

        let from_configs = find_from_attribute(&input).unwrap();
        assert_eq!(from_configs.len(), 2);
        assert!(from_configs[0].field_expressions.is_empty());
        assert!(from_configs[1].field_expressions.is_empty());
    }

    #[test]
    fn test_find_from_attribute_with_field_expressions() {
        let input: DeriveInput = parse_quote! {
            #[from(std::io::Error(kind: ErrorKind::Io, message: "IO error occurred".to_string()))]
            struct TestError {
                kind: ErrorKind,
                message: String,
                #[error]
                inner: OhnoCore,
            }
        };

        let from_configs = find_from_attribute(&input).unwrap();
        assert_eq!(from_configs.len(), 1);
        assert_field_keys(&from_configs[0], &["kind", "message"]);
    }

    #[test]
    fn test_find_from_attribute_with_field_expressions_for_tuple_error() {
        let input: DeriveInput = parse_quote! {
            #[from(std::io::Error(1: "IO error occurred".to_string()))]
            struct TestError(OhnoCore, String);
        };

        let from_configs = find_from_attribute(&input).unwrap();
        assert_eq!(from_configs.len(), 1);
        assert_field_keys(&from_configs[0], &["1"]);
    }

    #[test]
    fn test_find_from_attribute_with_invalid_field_for_tuple() {
        let input: DeriveInput = parse_quote! {
            #[from(std::io::Error("XYZ": "IO error occurred".to_string()))]
            struct TestError(OhnoCore, String);
        };

        expect_from_error(&input, "Only integer literals are supported for tuple field indices");
    }

    #[test]
    fn test_find_from_attribute_invalid_forms() {
        let cases = vec![
            (
                parse_quote! {
                    #[from()]
                    struct TestError {
                        #[error]
                        inner: OhnoCore,
                    }
                },
                "empty #[from()] attribute is not allowed",
            ),
            (
                parse_quote! {
                    #[from]
                    struct TestError {
                        #[error]
                        inner: OhnoCore,
                    }
                },
                "empty #[from] attribute is not allowed",
            ),
            (
                parse_quote! {
                    #[from = "Hello"]
                    struct TestError(OhnoCore, String);
                },
                "from attribute must be in the form",
            ),
        ];

        for (input, expected) in cases {
            expect_from_error(&input, expected);
        }
    }
}
