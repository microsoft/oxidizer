// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::{Data, DeriveInput, Expr, Fields, Result};

use crate::derive_error::attributes::DisplayAttribute;
use crate::utils::bail;

/// Parse display template to support field references like {`field_name`}
/// or format!-style with separate arguments
#[cfg_attr(test, mutants::skip)] // Baselined - we lack full test coverage of the {} escaping logic.
pub fn parse_display_template(display_attr: &DisplayAttribute, input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let mut result = String::new();
    let mut chars = display_attr.template.chars().peekable();
    let mut format_args = Vec::new();
    let mut arg_index = 0;

    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                if chars.peek() == Some(&'{') {
                    // Escaped brace: {{
                    chars.next();
                    result.push_str("{{");
                } else {
                    // Parse placeholder: {} or {field_name} or {field_name:format}
                    let (field_name, format_spec) = parse_field_reference(&mut chars);

                    let format_str = if format_spec.is_empty() {
                        "{}".to_string()
                    } else {
                        format!("{{:{format_spec}}}")
                    };
                    result.push_str(&format_str);

                    if field_name.is_empty() {
                        // Empty placeholder {}, use next argument from args list
                        if arg_index >= display_attr.args.len() {
                            bail!("Not enough arguments for format placeholders");
                        }
                        let arg = &display_attr.args[arg_index];
                        let arg_tokens = convert_expr_to_field_access(arg);
                        format_args.push(arg_tokens);
                        arg_index += 1;
                    } else {
                        // Named field reference like {field_name}
                        validate_field_exists(&field_name, input)?;
                        let field_ident = syn::Ident::new(&field_name, proc_macro2::Span::call_site());
                        format_args.push(quote! { &self.#field_ident });
                    }
                }
            }
            '}' if chars.peek() == Some(&'}') => {
                // Escaped closing brace: }}
                chars.next();
                result.push_str("}}");
            }
            _ => result.push(ch),
        }
    }

    // Check that all arguments were used
    if arg_index != display_attr.args.len() {
        bail!("Too many arguments for format placeholders");
    }

    Ok(generate_display_expression(&result, &format_args))
}

/// Convert expression to appropriate field access
fn convert_expr_to_field_access(expr: &Expr) -> proc_macro2::TokenStream {
    // Simply prefix any expression with &self.
    quote! { &self.#expr }
}

/// Extract field name from template between braces, handling format specifiers
fn parse_field_reference(chars: &mut std::iter::Peekable<std::str::Chars>) -> (String, String) {
    let mut field_name = String::new();
    let mut format_spec = String::new();
    let mut in_format = false;

    while let Some(&ch) = chars.peek() {
        if ch == '}' {
            chars.next();
            break;
        } else if ch == ':' && !in_format {
            in_format = true;
            chars.next(); // consume the ':'
        } else if in_format {
            format_spec.push(chars.next().unwrap());
        } else {
            field_name.push(chars.next().unwrap());
        }
    }

    (field_name, format_spec)
}

/// Validate that the field exists in the struct
fn validate_field_exists(field_name: &str, input: &DeriveInput) -> Result<()> {
    if !field_exists(field_name, input) {
        bail!("Field '{field_name}' not found in struct");
    }
    Ok(())
}

/// Generate the final display expression
fn generate_display_expression(result: &str, format_args: &[proc_macro2::TokenStream]) -> proc_macro2::TokenStream {
    if format_args.is_empty() {
        quote! { std::borrow::Cow::from(#result) }
    } else {
        quote! { std::borrow::Cow::from(format!(#result, #(#format_args),*)) }
    }
}

/// Check if a field exists in the struct
pub fn field_exists(field_name: &str, input: &DeriveInput) -> bool {
    let Data::Struct(data_struct) = &input.data else {
        return false;
    };

    let Fields::Named(fields) = &data_struct.fields else {
        return false;
    };

    fields
        .named
        .iter()
        .any(|field| field.ident.as_ref().is_some_and(|ident| ident == field_name))
}

#[cfg(test)]
mod tests {
    use syn::parse_quote; // removed Expr to avoid redundant import warning

    use super::*;

    // Helper to build DisplayAttribute
    fn da(template: &str, args: Vec<syn::Expr>) -> crate::derive_error::attributes::DisplayAttribute {
        crate::derive_error::attributes::DisplayAttribute {
            template: template.to_string(),
            args,
        }
    }
    // Helper to parse template quickly
    fn parse(template: &str, args: Vec<syn::Expr>, input: &DeriveInput) -> proc_macro2::TokenStream {
        parse_display_template(&da(template, args), input).unwrap()
    }

    #[test]
    fn test_parse_display_template_simple() {
        let input: DeriveInput = parse_quote! {
            struct TestError {
                path: String,
                #[error]
                inner: OhnoCore,
            }
        };
        let result = parse("Error with path: {path}", vec![], &input);
        let expected = quote! { std::borrow::Cow::from(format!("Error with path: {}", &self.path)) };
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_parse_display_template_no_fields() {
        let input: DeriveInput = parse_quote! {
            struct TestError { #[error] inner: OhnoCore }
        };
        let result = parse("Static error message", vec![], &input);
        let expected = quote! { std::borrow::Cow::from("Static error message") };
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_parse_display_template_with_args() {
        let input: DeriveInput = parse_quote! { struct TestError { data: Data, #[error] inner: OhnoCore } };
        let result = parse(
            "Invalid data: {} - {}",
            vec![parse_quote! { data.0 }, parse_quote! { data.1 }],
            &input,
        );
        let expected = quote! { std::borrow::Cow::from(format!("Invalid data: {} - {}", &self.data.0, &self.data.1)) };
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_field_exists_valid_and_invalid() {
        // Covers valid fields + invalid fields in one pass (previously two tests)
        let input: DeriveInput = parse_quote! {
            struct TestError { path: String, code: i32, #[error] inner: OhnoCore }
        };
        // Valid
        validate_field_exists("path", &input).unwrap();
        validate_field_exists("code", &input).unwrap();
        validate_field_exists("inner", &input).unwrap();
        // Invalid
        assert!(validate_field_exists("nonexistent", &input).is_err());
        assert!(validate_field_exists("inner2", &input).is_err());
    }

    #[test]
    fn test_field_exists_negative_struct_variants() {
        // Enum -> first pattern fails
        let enum_input: DeriveInput = parse_quote! { enum TestError { Variant1, Variant2 { field: String } } };
        assert!(!field_exists("field", &enum_input));
        assert!(!field_exists("any_field", &enum_input));

        // Tuple struct -> second pattern fails
        let tuple_input: DeriveInput = parse_quote! { struct TestError(String, i32); };
        assert!(!field_exists("0", &tuple_input));
        assert!(!field_exists("field", &tuple_input));

        // Unit struct -> second pattern fails (Fields::Unit)
        let unit_input: DeriveInput = parse_quote! { struct TestError; };
        assert!(!field_exists("field", &unit_input));
    }

    #[test]
    #[expect(clippy::literal_string_with_formatting_args, reason = "False positive")]
    fn test_parse_display_template_with_format_specifiers() {
        let input: DeriveInput = parse_quote! { struct TestError { errors: Vec<String>, #[error] inner: OhnoCore } };
        let result = parse("Failed to parse: {errors:?}", vec![], &input);
        let expected = quote! { std::borrow::Cow::from(format!("Failed to parse: {:?}", &self.errors)) };
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_parse_display_template_escaped_braces() {
        let input: DeriveInput = parse_quote! { struct TestError { field: String, #[error] inner: OhnoCore } };
        // Escaped opening braces
        let r1 = parse("Error: {{static}} with {field}", vec![], &input);
        let e1 = quote! { std::borrow::Cow::from(format!("Error: {{static}} with {}", &self.field)) };
        assert_eq!(r1.to_string(), e1.to_string());
        // Extra closing brace after placeholder
        let r2 = parse("Error: {field}} extra brace", vec![], &input);
        let e2 = quote! { std::borrow::Cow::from(format!("Error: {}} extra brace", &self.field)) };
        assert_eq!(r2.to_string(), e2.to_string());
        // Multiple escaped braces
        let r3 = parse("{{Error}}: {field} {{end}}", vec![], &input);
        let e3 = quote! { std::borrow::Cow::from(format!("{{Error}}: {} {{end}}", &self.field)) };
        assert_eq!(r3.to_string(), e3.to_string());
    }

    #[test]
    fn test_parse_display_template_with_method_calls() {
        let input: DeriveInput = parse_quote! { struct TestError { data: Data, #[error] inner: OhnoCore } };
        let result = parse(
            "Error: {} - {}",
            vec![parse_quote! { data.to_string() }, parse_quote! { data.len() }],
            &input,
        );
        assert_eq!(
            result.to_string(),
            "std :: borrow :: Cow :: from (format ! (\"Error: {} - {}\" , & self . data . to_string () , & self . data . len ()))"
        );
    }

    #[test]
    fn test_parse_display_template_with_nested_access() {
        let input: DeriveInput = parse_quote! { struct TestError { t: TupleType, #[error] inner: OhnoCore } };
        let result = parse(
            "Error: {} - {}",
            vec![parse_quote! { t.0.0.0.message() }, parse_quote! { t.0.0.0.m }],
            &input,
        );
        assert_eq!(
            result.to_string(),
            "std :: borrow :: Cow :: from (format ! (\"Error: {} - {}\" , & self . t . 0 . 0 . 0 . message () , & self . t . 0 . 0 . 0 . m))"
        );
    }

    #[test]
    fn test_parse_display_template_not_enough_arguments() {
        let input: DeriveInput = parse_quote! { struct TestError { data: Data, #[error] inner: OhnoCore } };
        let display_attr = da(
            "Error: {} - {} - {}",
            vec![parse_quote! { data.field1 }, parse_quote! { data.field2 }],
        );
        let result = parse_display_template(&display_attr, &input);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Not enough arguments for format placeholders");
    }

    #[test]
    fn test_parse_display_template_too_many_arguments() {
        let input: DeriveInput = parse_quote! { struct TestError { data: Data, #[error] inner: OhnoCore } };
        let display_attr = da(
            "Error: {} - {}",
            vec![
                parse_quote! { data.field1 },
                parse_quote! { data.field2 },
                parse_quote! { data.field3 },
            ],
        );
        let result = parse_display_template(&display_attr, &input);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Too many arguments for format placeholders");
    }

    #[test]
    fn test_parse_display_template_exact_argument_match() {
        let input: DeriveInput = parse_quote! { struct TestError { data: Data, #[error] inner: OhnoCore } };
        let display_attr = da("Error: {} - {}", vec![parse_quote! { data.field1 }, parse_quote! { data.field2 }]);
        parse_display_template(&display_attr, &input).unwrap();
    }
}
