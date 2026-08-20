// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::quote;
use syn::parse_quote;

use super::*;

#[test]
fn a_plain_template_renders_as_a_literal() {
    let rendered = Message::opaque("plain text".to_owned(), Vec::new()).render().to_string();
    assert_eq!(rendered, r#""plain text""#);
}

#[test]
fn a_template_with_braces_renders_through_format() {
    let rendered = Message::opaque("value {name}".to_owned(), Vec::new()).render().to_string();
    assert!(rendered.contains("format"), "{rendered}");
    assert!(rendered.contains(r#""value {name}""#), "{rendered}");
}

#[test]
fn arguments_force_a_format_call() {
    let rendered = Message::opaque("plain".to_owned(), vec![quote!(x)]).render().to_string();
    assert!(rendered.contains("format"), "{rendered}");
    assert!(rendered.ends_with("x)"), "{rendered}");
}

#[test]
fn a_literal_message_renders_without_format() {
    let rendered = Message::Literal("only { one brace".to_owned()).render().to_string();
    assert!(!rendered.contains("format"), "{rendered}");
}

#[test]
fn format_args_accept_a_bare_template() {
    let args: FormatArgs = parse_quote!("just text");
    assert_eq!(args.template.value(), "just text");
    assert!(args.arguments.is_empty());
}

#[test]
fn format_args_accept_a_template_and_arguments() {
    let args: FormatArgs = parse_quote!("text {} {}", first, second.len());
    assert_eq!(args.template.value(), "text {} {}");
    assert_eq!(args.arguments.len(), 2);
}

#[test]
fn format_args_accept_a_trailing_comma() {
    let args: FormatArgs = parse_quote!("text {}", first,);
    assert_eq!(args.arguments.len(), 1);
}

#[test]
fn format_args_reject_a_leading_expression() {
    let parsed = syn::parse2::<FormatArgs>(quote!(not_a_literal));
    let _ = parsed.expect_err("a leading expression must be rejected");
}
