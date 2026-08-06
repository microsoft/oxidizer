// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::{Data, DeriveInput, Expr, Ident, Index, Member, Result};

use crate::derive_error::attributes::DisplayAttribute;
use crate::derive_error::field_detection::is_generated_error_field;
use crate::utils::{bail, bail_spanned};

const SELF_SCOPED_ARGS: &str = "`#[display(...)]` positional arguments are implicitly scoped to `self`, so a field is referenced by its bare name, without a `self.` prefix";
const UNSUPPORTED_ROOT: &str = "`#[display(...)]` positional arguments are implicitly scoped to `self`, so each argument must be rooted in a field or method of `self`";

/// A parsed piece of a display template
///
/// Every part borrows from the template, which is possible because none of them is rewritten:
/// text, field names and format specifiers are all spelled in the template as they are used.
enum Segment<'a> {
    /// Literal text, already spelled the way `format!` expects it
    Text(&'a str),
    /// `{name}` or `{0}`, with its format specifier
    Field { name: &'a str, spec: &'a str },
    /// `{}`, with its format specifier
    Positional { spec: &'a str },
}

/// Split a display template into its segments
///
/// Parsing answers only what the template says. Whether a field exists, or whether the argument
/// count matches, is decided afterwards against the parsed form.
fn parse_template(template: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut chars = template.char_indices().peekable();
    let mut text_start = 0;

    while let Some((index, ch)) = chars.next() {
        if ch != '{' {
            continue;
        }

        // An escaped brace stays part of the surrounding text, spelled `{{` there as well
        if chars.next_if(|&(_, next)| next == '{').is_some() {
            continue;
        }

        if index > text_start {
            segments.push(Segment::Text(&template[text_start..index]));
        }

        // The placeholder runs to the next `}`, or to the end of an unterminated template
        let body_start = index + ch.len_utf8();
        let mut body_end = template.len();
        text_start = template.len();
        for (offset, ch) in chars.by_ref() {
            if ch == '}' {
                body_end = offset;
                text_start = offset + ch.len_utf8();
                break;
            }
        }

        let body = &template[body_start..body_end];
        let (name, spec) = body.split_once(':').unwrap_or((body, ""));
        segments.push(if name.is_empty() {
            Segment::Positional { spec }
        } else {
            Segment::Field { name, spec }
        });
    }

    if text_start < template.len() {
        segments.push(Segment::Text(&template[text_start..]));
    }

    segments
}

/// Append the `format!` placeholder a segment stands for
fn push_placeholder(result: &mut String, spec: &str) {
    if spec.is_empty() {
        result.push_str("{}");
    } else {
        result.push_str("{:");
        result.push_str(spec);
        result.push('}');
    }
}

/// Parse display template to support field references like `{field_name}`
/// or format!-style with separate arguments
///
/// See `docs/error_display.md`.
pub(crate) fn parse_display_template(display_attr: &DisplayAttribute, input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let mut result = String::new();
    let mut format_args = Vec::new();
    let mut arg_index = 0;

    for segment in parse_template(&display_attr.template) {
        match segment {
            Segment::Text(text) => result.push_str(text),
            Segment::Field { name, spec } => {
                push_placeholder(&mut result, spec);
                validate_field_exists(name, input, display_attr.template_span)?;
                let member = field_member(name);
                format_args.push(quote! { &self.#member });
            }
            Segment::Positional { spec } => {
                push_placeholder(&mut result, spec);

                let Some(arg) = display_attr.args.get(arg_index) else {
                    bail!(display_attr.template_span, "Not enough arguments for format placeholders");
                };
                format_args.push(convert_expr_to_field_access(arg, input)?);
                arg_index += 1;
            }
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
/// See `docs/error_display.md` for the scoping rules and why they are checked here.
fn convert_expr_to_field_access(expr: &Expr, input: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    validate_arg_root(expr, input)?;

    // Whether an argument can carry the prefix at all is decided by parsing the expansion, rather
    // than by enumerating the expression forms that may follow a dot
    let scoped = quote! { self.#expr };
    if syn::parse2::<Expr>(scoped.clone()).is_err() {
        bail_spanned!(expr, UNSUPPORTED_ROOT);
    }

    // The reference has to cover the whole argument. Without the parentheses it would bind to the
    // root alone, so `count as u64` would expand to `&self.count as u64`, casting the reference
    Ok(quote! { &(#scoped) })
}

/// Validate the field access a positional argument is rooted in
fn validate_arg_root(expr: &Expr, input: &DeriveInput) -> Result<()> {
    match root_of_expr(expr) {
        // `self.path` would expand to `&self.self.path`
        Expr::Path(path) if path.path.is_ident("self") => bail_spanned!(path, SELF_SCOPED_ARGS),
        Expr::Path(path) => match path.path.get_ident() {
            Some(ident) => validate_field_exists(&ident.to_string(), input, ident.span()),
            None => Ok(()),
        },
        Expr::Lit(literal) => validate_literal_root(literal, input),
        // Any other root names no field, so there is nothing to check here
        _ => Ok(()),
    }
}

/// Validate the tuple index a literal-rooted argument refers to
fn validate_literal_root(literal: &syn::ExprLit, input: &DeriveInput) -> Result<()> {
    match &literal.lit {
        syn::Lit::Int(index) => validate_field_exists(index.base10_digits(), input, index.span()),
        // Nested tuple access such as `0.1` lexes as a float; only its leading component names a
        // field of `self`, the rest reaches into the field's own type
        syn::Lit::Float(index) => match index.base10_digits().split_once('.') {
            Some((outer, _)) => validate_field_exists(outer, input, index.span()),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

/// Walk an expression down to the term the whole expression is rooted in
///
/// Every form here keeps the root in leftmost position, which is where `self.` lands. See
/// `docs/error_display.md`.
fn root_of_expr(expr: &Expr) -> &Expr {
    match expr {
        Expr::Field(inner) => root_of_expr(&inner.base),
        Expr::MethodCall(inner) => root_of_expr(&inner.receiver),
        Expr::Index(inner) => root_of_expr(&inner.expr),
        Expr::Binary(inner) => root_of_expr(&inner.left),
        Expr::Cast(inner) => root_of_expr(&inner.expr),
        Expr::Await(inner) => root_of_expr(&inner.base),
        Expr::Try(inner) => root_of_expr(&inner.expr),
        Expr::Range(inner) => inner.start.as_ref().map_or(expr, |start| root_of_expr(start)),
        _ => expr,
    }
}

/// Build the member used to access a field by name or by tuple index
fn field_member(field_name: &str) -> Member {
    field_name.parse::<usize>().map_or_else(
        |_| Member::Named(field_ident(field_name)),
        |index| Member::Unnamed(Index::from(index)),
    )
}

/// Build the identifier naming a field, keeping raw identifiers raw
///
/// `Ident::new` panics on the `r#` spelling, which would turn an ordinary error in a user's
/// template into a macro crash.
fn field_ident(field_name: &str) -> Ident {
    let span = proc_macro2::Span::call_site();
    field_name
        .strip_prefix("r#")
        .map_or_else(|| Ident::new(field_name, span), |bare| Ident::new_raw(bare, span))
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
    let names = referenceable_field_names(input).map(|name| format!("`{name}`")).collect::<Vec<_>>();
    if names.is_empty() {
        return "the error type has no fields that can be referenced".to_string();
    }

    format!("available fields: {}", names.join(", "))
}

/// The names a display template may reference, by name or by tuple index
///
/// The field injected by `#[ohno::error]` is left out; a core the user declared is kept. See
/// `docs/error_display.md`.
fn referenceable_field_names(input: &DeriveInput) -> impl Iterator<Item = String> + '_ {
    let fields = match &input.data {
        Data::Struct(data_struct) => Some(&data_struct.fields),
        _ => None,
    };

    fields
        .into_iter()
        // Positions are taken before filtering, so an index still names the field it did
        .flat_map(|fields| fields.iter().enumerate())
        .filter(|(_, field)| !is_generated_error_field(field))
        .map(|(index, field)| {
            field
                .ident
                .as_ref()
                .map_or_else(|| index.to_string(), ToString::to_string)
        })
}

/// Generate the final display expression
fn generate_display_expression(result: &str, format_args: &[proc_macro2::TokenStream]) -> proc_macro2::TokenStream {
    if format_args.is_empty() {
        quote! { std::borrow::Cow::from(#result) }
    } else {
        quote! { std::borrow::Cow::from(format!(#result, #(#format_args),*)) }
    }
}

/// Check if a field can be referenced from a display template, by name or by tuple index
pub(crate) fn field_exists(field_name: &str, input: &DeriveInput) -> bool {
    referenceable_field_names(input).any(|name| name == field_name)
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
    fn test_parse_display_template_raw_identifier_field() {
        // A raw identifier reaches the template as `r#type`, which `Ident::new` rejects. Building
        // the member from that spelling used to panic, turning a user's template into a crash
        let input: DeriveInput = parse_quote! {
            struct TestError {
                r#type: String,
                #[error]
                inner: OhnoCore,
            }
        };

        let result = parse("kind: {r#type}", vec![], &input);
        let expected = quote! { std::borrow::Cow::from(format!("kind: {}", &self.r#type)) };
        assert_eq!(result.to_string(), expected.to_string());

        // The bare spelling names no field, and the list offers the one that exists
        let message = parse_err("kind: {type}", vec![], &input);
        assert_eq!(
            message,
            "unknown field `type` in `#[display(...)]`, available fields: `r#type`, `inner`"
        );
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
        let expected = quote! { std::borrow::Cow::from(format!("Invalid data: {} - {}", &(self.data.0), &(self.data.1))) };
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
            "std :: borrow :: Cow :: from (format ! (\"Error: {} - {}\" , & (self . data . to_string ()) , & (self . data . len ())))"
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
            "std :: borrow :: Cow :: from (format ! (\"Error: {} - {}\" , & (self . t . 0 . 0 . 0 . message ()) , & (self . t . 0 . 0 . 0 . m)))"
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
        let input: DeriveInput =
            parse_quote! { struct TestError { path: PathBuf, #[doc = " ohno::generated-core@7f3d9c2a"] ohno_core: OhnoCore } };

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
        let input: DeriveInput =
            parse_quote! { struct TestError { path: PathBuf, code: i32, #[doc = " ohno::generated-core@7f3d9c2a"] ohno_core: OhnoCore } };

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
        let input: DeriveInput = parse_quote! { struct TestError { #[doc = " ohno::generated-core@7f3d9c2a"] ohno_core: OhnoCore } };
        let message = parse_err("bad path: {path}", vec![], &input);
        assert_eq!(
            message,
            "unknown field `path` in `#[display(...)]`, the error type has no fields that can be referenced"
        );
    }

    #[test]
    fn test_tuple_struct_fields_are_referenceable_by_index() {
        let input: DeriveInput = parse_quote! { struct TestError(PathBuf, #[doc = " ohno::generated-core@7f3d9c2a"] OhnoCore); };

        let result = parse("bad path: {0}", vec![], &input);
        let expected = quote! { std::borrow::Cow::from(format!("bad path: {}", &self.0)) };
        assert_eq!(result.to_string(), expected.to_string());

        let result = parse("bad path: {}", vec![parse_quote! { 0.display() }], &input);
        let expected = quote! { std::borrow::Cow::from(format!("bad path: {}", &(self.0.display()))) };
        assert_eq!(result.to_string(), expected.to_string());

        // The injected OhnoCore is the second field, so only index 0 is offered
        let message = parse_err("bad path: {5}", vec![], &input);
        assert_eq!(message, "unknown field `5` in `#[display(...)]`, available fields: `0`");
    }

    #[test]
    fn test_index_of_the_generated_error_field_is_not_referenceable() {
        // Index 1 is in range but holds the injected OhnoCore, whose Display prints the error's
        // own chain; referencing it is a mistake rather than a way to reach the core
        let input: DeriveInput = parse_quote! { struct TestError(PathBuf, #[doc = " ohno::generated-core@7f3d9c2a"] OhnoCore); };

        for message in [
            parse_err("bad path: {1}", vec![], &input),
            parse_err("bad path: {}", vec![parse_quote! { 1 }], &input),
        ] {
            assert_eq!(message, "unknown field `1` in `#[display(...)]`, available fields: `0`");
        }
    }

    #[test]
    fn test_name_of_the_generated_error_field_is_not_referenceable() {
        let input: DeriveInput =
            parse_quote! { struct TestError { path: PathBuf, #[doc = " ohno::generated-core@7f3d9c2a"] ohno_core: OhnoCore } };

        for message in [
            parse_err("bad path: {ohno_core}", vec![], &input),
            parse_err("bad path: {}", vec![parse_quote! { ohno_core }], &input),
        ] {
            assert_eq!(message, "unknown field `ohno_core` in `#[display(...)]`, available fields: `path`");
        }
    }

    #[test]
    fn test_positional_argument_rooted_in_a_method_call_on_self_is_left_alone() {
        let input: DeriveInput =
            parse_quote! { struct TestError { path: PathBuf, #[doc = " ohno::generated-core@7f3d9c2a"] ohno_core: OhnoCore } };

        // A method call on `self` is not a field access, so there is no field to validate
        let result = parse("bad path: {}", vec![parse_quote! { describe() }], &input);
        let expected = quote! { std::borrow::Cow::from(format!("bad path: {}", &(self.describe()))) };
        assert_eq!(result.to_string(), expected.to_string());
    }

    #[test]
    fn test_nested_tuple_index_is_validated_against_its_leading_component() {
        // `0.1` lexes as a float, but names field 0 of `self` and field 1 of its type
        let input: DeriveInput = parse_quote! { struct TestError(Inner, #[doc = " ohno::generated-core@7f3d9c2a"] OhnoCore); };

        let result = parse("bad: {}", vec![parse_quote! { 0.1 }], &input);
        let expected = quote! { std::borrow::Cow::from(format!("bad: {}", &(self.0.1))) };
        assert_eq!(result.to_string(), expected.to_string());

        // Index 1 is the injected OhnoCore, so it is caught here rather than by rustc
        let message = parse_err("bad: {}", vec![parse_quote! { 1.0 }], &input);
        assert_eq!(message, "unknown field `1` in `#[display(...)]`, available fields: `0`");
    }

    #[test]
    fn test_argument_root_is_found_through_leftmost_position() {
        // `self.` lands before the leftmost term, so these all keep the root a field of `self`
        let input: DeriveInput =
            parse_quote! { struct TestError { count: u32, #[doc = " ohno::generated-core@7f3d9c2a"] ohno_core: OhnoCore } };

        // The reference covers the whole argument, so the cast and the multiplication apply to the
        // field's value rather than to a reference to it
        let result = parse("total: {}", vec![parse_quote! { count * 2 }], &input);
        let expected = quote! { std::borrow::Cow::from(format!("total: {}", &(self.count * 2))) };
        assert_eq!(result.to_string(), expected.to_string());

        let result = parse("total: {}", vec![parse_quote! { count as u64 }], &input);
        let expected = quote! { std::borrow::Cow::from(format!("total: {}", &(self.count as u64))) };
        assert_eq!(result.to_string(), expected.to_string());

        // The root is still validated through those forms
        for arg in [
            parse_quote! { cnt * 2 },
            parse_quote! { cnt as u64 },
            parse_quote! { cnt..8 },
            parse_quote! { cnt.await },
            parse_quote! { cnt? },
        ] {
            let message = parse_err("total: {}", vec![arg], &input);
            assert_eq!(message, "unknown field `cnt` in `#[display(...)]`, available fields: `count`");
        }
    }

    #[test]
    fn test_argument_rooted_in_something_that_cannot_follow_self_is_rejected() {
        // Each of these would otherwise expand to code that does not parse, such as
        // `&self.Self::LABEL.len()`, and be reported by rustc against generated code
        let input: DeriveInput =
            parse_quote! { struct TestError { path: PathBuf, #[doc = " ohno::generated-core@7f3d9c2a"] ohno_core: OhnoCore } };

        for arg in [
            parse_quote! { Self::LABEL.len() },
            parse_quote! { std::env::consts::OS.len() },
            parse_quote! { Self::describe() },
            parse_quote! { "prefix".len() },
            parse_quote! { 'c'.len_utf8() },
            parse_quote! { 1e3 },
            parse_quote! { format!("x").len() },
            parse_quote! { (path).display() },
        ] {
            let message = parse_err("bad path: {}", vec![arg], &input);
            assert_eq!(message, UNSUPPORTED_ROOT);
        }
    }
}
