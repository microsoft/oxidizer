// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[display(...)]` into a [`Message`].
//!
//! The macro reports a bad template or a bad argument itself, rather than letting the expansion
//! fail. Otherwise a bad argument reaches `rustc` as a field access in code the user cannot see,
//! and the field added by `#[ohno::error]` appears in `rustc`'s own list of available fields.

pub(crate) mod argument;
pub(crate) mod template;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, Member};

use super::ast::{AstField, DisplayAttr};
use super::parse::member_name;
use crate::diagnostics::Errors;
use crate::message::Message;

/// The fields a template and its arguments may name.
pub(crate) struct Referenceable<'a>(Vec<&'a Member>);

impl<'a> Referenceable<'a> {
    /// Collects the fields the user wrote, in declaration order.
    pub(crate) fn new(fields: &'a [AstField]) -> Self {
        Self(
            fields
                .iter()
                .filter(|field| field.is_referenceable())
                .map(|field| &field.member)
                .collect(),
        )
    }

    /// The field spelled `name`, if there is one.
    fn resolve(&self, name: &str) -> Option<&'a Member> {
        self.0.iter().copied().find(|member| member_name(member) == name)
    }

    /// The fields a diagnostic offers, backticked and comma separated.
    fn available(&self) -> String {
        self.0
            .iter()
            .map(|member| format!("`{}`", member_name(member)))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The diagnostic for a name that is not one of these fields.
    fn unknown(&self, name: &str) -> String {
        format!(
            "unknown field `{name}` in `#[display(...)]`, available fields: {}",
            self.available()
        )
    }
}

/// Lowers `display` into a [`Message`], reporting everything that is not allowed.
///
/// Returns `None` when the template could not be split, in which case its placeholders and the
/// argument count say nothing and are not checked: a repaired template would only produce faults
/// invented by the repair.
pub(crate) fn lower(display: &DisplayAttr, fields: &Referenceable<'_>, errors: &mut Errors) -> Option<Message> {
    let raw = display.template.value();

    let segments = match template::split(&raw) {
        Ok(segments) => segments,
        Err(fault) => {
            errors.add(&display.template, fault.message());
            return None;
        }
    };

    let mut lowered = String::with_capacity(raw.len());
    let mut arguments = Vec::new();
    let mut positional = display.arguments.iter();

    for segment in segments {
        match segment {
            template::Segment::Literal(text) => lowered.push_str(text),
            template::Segment::Placeholder(placeholder) => {
                lowered.push_str(&placeholder.lowered());

                match placeholder.name {
                    Some(name) => match fields.resolve(name) {
                        Some(member) => arguments.push(quote!(&self.#member)),
                        None => errors.add(&display.template, fields.unknown(name)),
                    },
                    None => match positional.next() {
                        Some(expr) => arguments.push(scope_to_self(expr, fields, errors)),
                        None => errors.add(
                            &display.template,
                            "`#[display(...)]` template has more `{}` placeholders than arguments",
                        ),
                    },
                }
            }
        }
    }

    for unused in positional {
        errors.add(unused, "`#[display(...)]` argument is not consumed by any `{}` placeholder");
    }

    Some(if arguments.is_empty() {
        Message::Literal(unescape(&lowered))
    } else {
        Message::Formatted {
            template: lowered,
            arguments,
        }
    })
}

/// Applies the implicit `self.` prefix to a positional argument.
///
/// The result is wrapped as `&(...)`. The parentheses are load-bearing: a bare `&self.<argument>`
/// binds the reference to the leftmost term alone, so `count as u64` would cast the reference
/// rather than the field, and `count * 2` would multiply it.
fn scope_to_self(expr: &Expr, fields: &Referenceable<'_>, errors: &mut Errors) -> TokenStream {
    match argument::root(expr) {
        argument::Root::SelfKeyword(term) => errors.add(term, argument::SELF_PREFIXED),
        argument::Root::Unsupported => errors.add(expr, argument::UNSUPPORTED_ROOT),
        root => {
            let name = root.field_name().unwrap_or_default();
            if fields.resolve(name).is_none() {
                // The root term is the smallest thing carrying the fault, so it is what the
                // diagnostic underlines.
                match root {
                    argument::Root::Name(_, term) | argument::Root::Index(_, term) => errors.add(term, fields.unknown(name)),
                    argument::Root::SelfKeyword(_) | argument::Root::Unsupported => unreachable!("handled above"),
                }
            }
        }
    }

    quote!(&(self.#expr))
}

/// Resolves `{{` and `}}` escapes, for a message that no longer goes through `format!`.
fn unescape(template: &str) -> String {
    template.replace("{{", "{").replace("}}", "}")
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;
    use crate::derive_error::ast::Ast;
    use crate::derive_error::parse;

    fn ast_of(input: syn::DeriveInput) -> Ast {
        let mut errors = Errors::default();
        let ast = parse::parse(input, &mut errors).expect("the input parses");
        assert!(errors.is_empty());
        ast
    }

    /// Lowers the `#[display(...)]` of `input`, returning the rendered message and the faults.
    fn lower_of(input: syn::DeriveInput) -> (Option<String>, String) {
        let ast = ast_of(input);
        let fields = Referenceable::new(&ast.fields);
        let mut errors = Errors::default();
        let message = lower(ast.display.as_ref().expect("a display attribute"), &fields, &mut errors);
        (
            message.map(|message| message.render().to_string()),
            errors.into_compile_error().to_string(),
        )
    }

    #[test]
    fn a_static_template_lowers_to_a_literal() {
        let (message, faults) = lower_of(parse_quote! {
            #[display("Static error message")]
            struct T { inner: ohno::OhnoCore }
        });

        assert!(faults.is_empty(), "{faults}");
        assert_eq!(message.as_deref(), Some(r#""Static error message""#));
    }

    #[test]
    fn escapes_are_resolved_for_a_literal_message() {
        let (message, faults) = lower_of(parse_quote! {
            #[display("a {{literal}} brace")]
            struct T { inner: ohno::OhnoCore }
        });

        assert!(faults.is_empty(), "{faults}");
        assert_eq!(message.as_deref(), Some(r#""a {literal} brace""#));
    }

    #[test]
    fn a_named_placeholder_becomes_a_field_access() {
        let (message, faults) = lower_of(parse_quote! {
            #[display("failed for {path}")]
            struct T { path: String, inner: ohno::OhnoCore }
        });

        assert!(faults.is_empty(), "{faults}");
        let message = message.expect("a message");
        assert!(message.contains(r#""failed for {}""#), "{message}");
        assert!(message.contains("self . path"), "{message}");
    }

    #[test]
    #[expect(clippy::literal_string_with_formatting_args, reason = "the format spec is the subject of the test")]
    fn a_format_spec_survives_lowering() {
        let (message, faults) = lower_of(parse_quote! {
            #[display("rules: {rules:?}")]
            struct T { rules: Vec<String>, inner: ohno::OhnoCore }
        });

        assert!(faults.is_empty(), "{faults}");
        assert!(message.expect("a message").contains(r#""rules: {:?}""#));
    }

    #[test]
    fn a_positional_argument_is_scoped_and_parenthesized() {
        let (message, faults) = lower_of(parse_quote! {
            #[display("{}", count * 2)]
            struct T { count: u32, inner: ohno::OhnoCore }
        });

        assert!(faults.is_empty(), "{faults}");
        let message = message.expect("a message");
        assert!(message.contains("& (self . count * 2)"), "{message}");
    }

    #[test]
    fn a_tuple_field_is_named_by_index() {
        let (message, faults) = lower_of(parse_quote! {
            #[display("{0} and {}", 1.abs())]
            struct T(String, i32, ohno::OhnoCore);
        });

        assert!(faults.is_empty(), "{faults}");
        assert!(message.expect("a message").contains("self . 0"));
    }

    #[test]
    fn the_added_core_is_not_referenceable() {
        let (_, faults) = lower_of(parse_quote! {
            #[display("bad path: {1}")]
            struct T(String, #[doc = " ohno::generated-core@7f3d9c2a"] ohno::OhnoCore);
        });

        assert!(faults.contains("unknown field `1`"), "{faults}");
        assert!(faults.contains("available fields: `0`"), "{faults}");
    }

    #[test]
    fn a_declared_core_stays_referenceable() {
        let (_, faults) = lower_of(parse_quote! {
            #[display("{path}, carrying {carried}")]
            struct T {
                path: String,
                carried: ohno::OhnoCore,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore,
            }
        });

        assert!(faults.is_empty(), "{faults}");
    }

    #[test]
    fn an_unknown_placeholder_lists_the_available_fields() {
        let (_, faults) = lower_of(parse_quote! {
            #[display("bad path: {pth}")]
            struct T {
                path: String,
                code: u32,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore,
            }
        });

        assert!(
            faults.contains("unknown field `pth` in `#[display(...)]`, available fields: `path`, `code`"),
            "{faults}"
        );
    }

    #[test]
    fn a_declared_core_is_offered_as_an_available_field() {
        let (_, faults) = lower_of(parse_quote! {
            #[display("bad path: {pth}")]
            struct T { path: String, inner: ohno::OhnoCore }
        });

        assert!(faults.contains("available fields: `path`, `inner`"), "{faults}");
    }

    #[test]
    fn an_unknown_argument_root_is_reported() {
        let (_, faults) = lower_of(parse_quote! {
            #[display("bad path: {}", pth.display())]
            struct T { path: String, code: u32, inner: ohno::OhnoCore }
        });

        assert!(faults.contains("unknown field `pth`"), "{faults}");
    }

    #[test]
    fn a_raw_identifier_is_offered_with_its_prefix() {
        let (_, faults) = lower_of(parse_quote! {
            #[display("bad: {typ}")]
            struct T {
                r#type: String,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore,
            }
        });

        assert!(faults.contains("available fields: `r#type`"), "{faults}");
    }

    #[test]
    fn a_self_prefixed_argument_is_reported() {
        let (_, faults) = lower_of(parse_quote! {
            #[display("bad path: {}", self.path.display())]
            struct T { path: String, inner: ohno::OhnoCore }
        });

        assert!(faults.contains("without a `self.` prefix"), "{faults}");
    }

    #[test]
    fn an_unsupported_argument_root_is_reported() {
        let (_, faults) = lower_of(parse_quote! {
            #[display("bad: {}", Self::LABEL.len())]
            struct T { inner: ohno::OhnoCore }
        });

        assert!(faults.contains("rooted in a field or method of `self`"), "{faults}");
    }

    #[test]
    fn an_unbalanced_brace_stops_the_lowering() {
        let (message, faults) = lower_of(parse_quote! {
            #[display("bad path: {path")]
            struct T { path: String, inner: ohno::OhnoCore }
        });

        assert!(message.is_none());
        assert!(faults.contains("with no matching `}`"), "{faults}");
    }

    #[test]
    fn a_stray_closing_brace_stops_the_lowering() {
        let (message, faults) = lower_of(parse_quote! {
            #[display("bad path: path}")]
            struct T { path: String, inner: ohno::OhnoCore }
        });

        assert!(message.is_none());
        assert!(faults.contains("with no matching `{`"), "{faults}");
    }

    #[test]
    fn too_few_arguments_are_reported() {
        let (_, faults) = lower_of(parse_quote! {
            #[display("{} {}", path)]
            struct T { path: String, inner: ohno::OhnoCore }
        });

        assert!(faults.contains("more `{}` placeholders than arguments"), "{faults}");
    }

    #[test]
    fn an_unconsumed_argument_is_reported() {
        let (_, faults) = lower_of(parse_quote! {
            #[display("{}", path, code)]
            struct T { path: String, code: u32, inner: ohno::OhnoCore }
        });

        assert!(faults.contains("not consumed by any `{}` placeholder"), "{faults}");
    }

    #[test]
    fn every_fault_in_one_template_is_reported_together() {
        let (_, faults) = lower_of(parse_quote! {
            #[display("{one} {two}", extra)]
            struct T { path: String, inner: ohno::OhnoCore }
        });

        assert_eq!(faults.matches("compile_error").count(), 3, "{faults}");
    }
}
