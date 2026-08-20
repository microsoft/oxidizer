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
#[derive(Debug)]
pub(crate) struct Referenceable<'a>(Vec<&'a Member>);

impl<'a> Referenceable<'a> {
    /// Collects the fields the user wrote, in declaration order.
    #[must_use]
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

    /// The fields a diagnostic offers, backticked and comma separated, or why it offers none.
    fn available(&self) -> String {
        if self.0.is_empty() {
            return "the error type has no fields that can be referenced".to_owned();
        }

        let names = self.0.iter().map(|member| format!("`{}`", member_name(member))).collect::<Vec<_>>();

        format!("available fields: {}", names.join(", "))
    }

    /// The diagnostic for a name that is not one of these fields.
    fn unknown(&self, name: &str) -> String {
        format!("unknown field `{name}` in `#[display(...)]`, {}", self.available())
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
        // A method of `self` names no field, so there is nothing to resolve.
        argument::Root::Method => {}
        // The root term is the smallest thing carrying the fault, so it is what the diagnostic
        // underlines.
        argument::Root::Name(name, term) | argument::Root::Index(name, term) => {
            if fields.resolve(&name).is_none() {
                errors.add(term, fields.unknown(&name));
            }
        }
    }

    quote!(&(self.#expr))
}

/// Resolves `{{` and `}}` escapes, for a message that no longer goes through `format!`.
fn unescape(template: &str) -> String {
    template.replace("{{", "{").replace("}}", "}")
}
