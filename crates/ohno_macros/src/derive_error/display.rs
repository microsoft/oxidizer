// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Expr, Fields, Ident, Index, Member, Result};

use crate::derive_error::attributes::DisplayAttribute;
use crate::derive_error::field_detection::is_generated_error_field;
use crate::utils::bail;

const SELF_SCOPED_ARGS: &str = "`#[display(...)]` positional arguments are implicitly scoped to `self`, so the `self.` prefix must be omitted: write `path.display()` instead of `self.path.display()`";

/// Parse display template to support field references like `{field_name}`
/// or format!-style with separate arguments
pub(crate) fn parse_display_template(display_attr: &DisplayAttribute, input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
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
                            bail!(display_attr.template_span, "Not enough arguments for format placeholders");
                        }
                        let arg = &display_attr.args[arg_index];
                        let arg_tokens = convert_expr_to_field_access(arg, input)?;
                        format_args.push(arg_tokens);
                        arg_index += 1;
                    } else {
                        // Named field reference like {field_name} or tuple index like {0}
                        validate_field_exists(&field_name, input, display_attr.template_span)?;
                        let member = field_member(&field_name);
                        format_args.push(quote! { &self.#member });
                    }
                }
            }
            // Note: `}}` does not need a dedicated arm. The `_` arm pushes each `}`
            // verbatim, producing the required `}}` sequence in the result format
            // string, which `format!` interprets as a literal `}`.
            _ => result.push(ch),
        }
    }

    // Check that all arguments were used
    if arg_index != display_attr.args.len() {
        bail!(display_attr.template_span, "Too many arguments for format placeholders");
    }

    Ok(generate_display_expression(&result, &format_args))
}

/// Convert expression to appropriate field access
///
/// Positional arguments are implicitly scoped to `self`, so the expression is prefixed with
/// `&self.`. Its root is validated first: otherwise a mis-scoped or misspelled argument is only
/// caught later by `rustc`, which reports it against the expanded struct and leaks the
/// macro-injected `OhnoCore` field into the diagnostic.
fn convert_expr_to_field_access(expr: &Expr, input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    validate_arg_root(expr, input)?;
    Ok(quote! { &self.#expr })
}

/// Validate the field access a positional argument is rooted in
fn validate_arg_root(expr: &Expr, input: &DeriveInput) -> Result<()> {
    match root_of_expr(expr) {
        // `self.path` would expand to `&self.self.path`
        Expr::Path(path) if path.path.is_ident("self") => bail!(expr.span(), SELF_SCOPED_ARGS),
        Expr::Path(path) => match path.path.get_ident() {
            Some(ident) => validate_field_exists(&ident.to_string(), input, ident.span()),
            None => Ok(()),
        },
        // Tuple field access such as `0` in `#[display("{}", 0)]`
        Expr::Lit(literal) => match &literal.lit {
            syn::Lit::Int(index) => validate_field_exists(index.base10_digits(), input, index.span()),
            _ => Ok(()),
        },
        // Anything else is not a field access
        _ => Ok(()),
    }
}

/// Walk an expression down to the term the whole expression is rooted in
fn root_of_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Field(inner) => root_of_expr(&inner.base),
        Expr::MethodCall(inner) => root_of_expr(&inner.receiver),
        Expr::Index(inner) => root_of_expr(&inner.expr),
        _ => expr,
    }
}

/// Build the member used to access a field by name or by tuple index
fn field_member(field_name: &str) -> Member {
    field_name.parse::<usize>().map_or_else(
        |_| Member::Named(Ident::new(field_name, proc_macro2::Span::call_site())),
        |index| Member::Unnamed(Index::from(index)),
    )
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
            chars.next();
        } else if in_format {
            format_spec.push(ch);
            chars.next();
        } else {
            field_name.push(ch);
            chars.next();
        }
    }

    (field_name, format_spec)
}

/// Validate that the field exists in the struct
fn validate_field_exists(field_name: &str, input: &DeriveInput, span: proc_macro2::Span) -> Result<()> {
    if !field_exists(field_name, input) {
        let available = describe_referenceable_fields(input);
        bail!(span, "unknown field `{field_name}` in `#[display(...)]`, {available}");
    }
    Ok(())
}

/// Describe the fields a display template is allowed to reference
fn describe_referenceable_fields(input: &DeriveInput) -> String {
    let names = referenceable_field_names(input);
    if names.is_empty() {
        return "the error type has no fields that can be referenced".to_string();
    }

    let names = names.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ");
    format!("available fields: {names}")
}

/// Collect the names of the fields the user declared
///
/// The `OhnoCore` field injected by `#[ohno::error]` is left out, as the user never wrote it and
/// would not recognize it in a diagnostic. A core field the user declared themselves is kept.
fn referenceable_field_names(input: &DeriveInput) -> Vec<String> {
    let Data::Struct(data_struct) = &input.data else {
        return Vec::new();
    };

    match &data_struct.fields {
        Fields::Named(fields) => fields
            .named
            .iter()
            .filter(|field| !is_generated_error_field(field))
            .filter_map(|field| field.ident.as_ref().map(ToString::to_string))
            .collect(),
        Fields::Unnamed(fields) => fields
            .unnamed
            .iter()
            .enumerate()
            .filter(|(_, field)| !is_generated_error_field(field))
            .map(|(index, _)| index.to_string())
            .collect(),
        Fields::Unit => Vec::new(),
    }
}

/// Generate the final display expression
fn generate_display_expression(result: &str, format_args: &[proc_macro2::TokenStream]) -> proc_macro2::TokenStream {
    if format_args.is_empty() {
        quote! { std::borrow::Cow::from(#result) }
    } else {
        quote! { std::borrow::Cow::from(format!(#result, #(#format_args),*)) }
    }
}

/// Check if a field exists in the struct, by name for named fields and by index for tuple structs
pub(crate) fn field_exists(field_name: &str, input: &DeriveInput) -> bool {
    let Data::Struct(data_struct) = &input.data else {
        return false;
    };

    match &data_struct.fields {
        Fields::Named(fields) => fields
            .named
            .iter()
            .any(|field| field.ident.as_ref().is_some_and(|ident| ident == field_name)),
        Fields::Unnamed(fields) => field_name.parse::<usize>().is_ok_and(|index| index < fields.unnamed.len()),
        Fields::Unit => false,
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote; // removed Expr to avoid redundant import warning

    use super::*;

    // Helper to build DisplayAttribute
    fn da(template: &str, args: Vec<syn::Expr>) -> crate::derive_error::attributes::DisplayAttribute {
        crate::derive_error::attributes::DisplayAttribute {
            template: template.to_string(),
            template_span: proc_macro2::Span::call_site(),
            args,
        }
    }
    // Helper to parse template quickly
    fn parse(template: &str, args: Vec<syn::Expr>, input: &DeriveInput) -> proc_macro2::TokenStream {
        parse_display_template(&da(template, args), input).unwrap()
    }

    // Helper to validate a field reference with a throwaway span
    fn validate(field_name: &str, input: &DeriveInput) -> Result<()> {
        validate_field_exists(field_name, input, proc_macro2::Span::call_site())
    }

    // Helper to get the error message produced for a template
    fn parse_err(template: &str, args: Vec<syn::Expr>, input: &DeriveInput) -> String {
        parse_display_template(&da(template, args), input).unwrap_err().to_string()
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
        validate("path", &input).unwrap();
        validate("code", &input).unwrap();
        validate("inner", &input).unwrap();
        // Invalid
        assert!(validate("nonexistent", &input).is_err());
        assert!(validate("inner2", &input).is_err());
    }

    #[test]
    fn test_field_exists_negative_struct_variants() {
        // Enum -> first pattern fails
        let enum_input: DeriveInput = parse_quote! { enum TestError { Variant1, Variant2 { field: String } } };
        assert!(!field_exists("field", &enum_input));
        assert!(!field_exists("any_field", &enum_input));

        // Tuple struct -> fields are addressed by index
        let tuple_input: DeriveInput = parse_quote! { struct TestError(String, i32); };
        assert!(field_exists("0", &tuple_input));
        assert!(field_exists("1", &tuple_input));
        assert!(!field_exists("2", &tuple_input));
        assert!(!field_exists("field", &tuple_input));

        // Unit struct -> no fields at all
        let unit_input: DeriveInput = parse_quote! { struct TestError; };
        assert!(!field_exists("field", &unit_input));
        assert!(!field_exists("0", &unit_input));
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

    #[test]
    fn test_positional_argument_with_self_prefix_is_rejected() {
        let input: DeriveInput = parse_quote! { struct TestError { path: PathBuf, #[error(generated)] ohno_core: OhnoCore } };

        // The spelling `thiserror` documents, which would expand to `&self.self.path`
        for arg in [
            parse_quote! { self.path },
            parse_quote! { self.path.display() },
            parse_quote! { self.path.as_os_str().len() },
            parse_quote! { self.paths[0].display() },
            parse_quote! { self },
        ] {
            let message = parse_err("bad path: {}", vec![arg], &input);
            assert_eq!(message, SELF_SCOPED_ARGS);
        }
    }

    #[test]
    fn test_unknown_field_reports_available_fields_without_the_error_core() {
        let input: DeriveInput = parse_quote! { struct TestError { path: PathBuf, code: i32, #[error(generated)] ohno_core: OhnoCore } };

        // Named reference and positional argument report the same thing
        for message in [
            parse_err("bad path: {pth}", vec![], &input),
            parse_err("bad path: {}", vec![parse_quote! { pth.display() }], &input),
            parse_err("bad path: {}", vec![parse_quote! { pth[0].display() }], &input),
        ] {
            assert_eq!(
                message,
                "unknown field `pth` in `#[display(...)]`, available fields: `path`, `code`"
            );
            assert!(!message.contains("ohno_core"));
        }
    }

    #[test]
    fn test_unknown_field_reports_a_core_field_the_user_declared() {
        // Only the field `#[ohno::error]` injects is hidden; this one is the user's to reference
        let input: DeriveInput = parse_quote! { struct TestError { path: PathBuf, #[error] inner: OhnoCore } };
        let message = parse_err("bad path: {pth}", vec![], &input);
        assert_eq!(
            message,
            "unknown field `pth` in `#[display(...)]`, available fields: `path`, `inner`"
        );
    }

    #[test]
    fn test_unknown_field_on_error_type_without_referenceable_fields() {
        let input: DeriveInput = parse_quote! { struct TestError { #[error(generated)] ohno_core: OhnoCore } };
        let message = parse_err("bad path: {path}", vec![], &input);
        assert_eq!(
            message,
            "unknown field `path` in `#[display(...)]`, the error type has no fields that can be referenced"
        );
    }

    #[test]
    fn test_tuple_struct_fields_are_referenceable_by_index() {
        let input: DeriveInput = parse_quote! { struct TestError(PathBuf, #[error(generated)] OhnoCore); };

        let result = parse("bad path: {0}", vec![], &input);
        let expected = quote! { std::borrow::Cow::from(format!("bad path: {}", &self.0)) };
        assert_eq!(result.to_string(), expected.to_string());

        let result = parse("bad path: {}", vec![parse_quote! { 0.display() }], &input);
        let expected = quote! { std::borrow::Cow::from(format!("bad path: {}", &self.0.display())) };
        assert_eq!(result.to_string(), expected.to_string());

        // The injected OhnoCore is the second field, so only index 0 is offered
        let message = parse_err("bad path: {5}", vec![], &input);
        assert_eq!(message, "unknown field `5` in `#[display(...)]`, available fields: `0`");
    }

    #[test]
    fn test_positional_argument_that_is_not_a_field_access_is_left_alone() {
        let input: DeriveInput = parse_quote! { struct TestError { path: PathBuf, #[error(generated)] ohno_core: OhnoCore } };

        // A method call on `self` is not a field access, so it is not validated here
        let result = parse("bad path: {}", vec![parse_quote! { describe() }], &input);
        let expected = quote! { std::borrow::Cow::from(format!("bad path: {}", &self.describe())) };
        assert_eq!(result.to_string(), expected.to_string());
    }
}
