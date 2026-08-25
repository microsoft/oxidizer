// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `DeriveInput` into [`Ast`].
//!
//! This phase answers one question per attribute: can it be read? An attribute that cannot is
//! reported here and left out of the `Ast`, so validation has nothing to check for it and reports
//! only faults it can actually see.

use proc_macro2::{Delimiter, Spacing};
use syn::buffer::Cursor;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Attribute, Data, DeriveInput, Expr, Fields, Index, Member, Meta, Token, Type};

use super::ast::{Ast, AstField, DisplayAttr, FromAttr, FromOverride, Style};
use crate::diagnostics::Errors;
use crate::marker;
use crate::message::FormatArgs;

/// Decodes `input` into an [`Ast`].
///
/// Returns `None` only when the input is not an error type at all, in which case a fault has been
/// recorded. Faults found while decoding individual attributes are recorded without giving up.
pub(crate) fn parse(input: DeriveInput, errors: &mut Errors) -> Option<Ast> {
    let DeriveInput {
        attrs,
        ident,
        generics,
        data,
        ..
    } = input;

    let (style, fields) = parse_fields(&ident, data, errors)?;

    let mut ast = Ast {
        ident,
        generics,
        style,
        fields,
        display: None,
        conversions: Vec::new(),
        no_debug: false,
        no_constructors: false,
    };

    for attr in &attrs {
        parse_struct_attribute(attr, &mut ast, errors);
    }

    Some(ast)
}

/// Reads the struct's own attributes into `ast`.
fn parse_struct_attribute(attr: &Attribute, ast: &mut Ast, errors: &mut Errors) {
    if attr.path().is_ident("display") {
        if ast.display.is_some() {
            errors.add(&attr.meta, "only one `#[display(...)]` may be given, and this is the second");
            return;
        }

        match attr.parse_args::<FormatArgs>() {
            Ok(args) => {
                ast.display = Some(DisplayAttr {
                    template: args.template,
                    arguments: args.arguments,
                });
            }
            Err(error) => errors.combine(error),
        }
    } else if attr.path().is_ident("from") {
        parse_from_attribute(attr, ast, errors);
    } else if attr.path().is_ident("no_debug") {
        check_bare_marker(attr, "no_debug", errors);
        ast.no_debug = true;
    } else if attr.path().is_ident("no_constructors") {
        check_bare_marker(attr, "no_constructors", errors);
        ast.no_constructors = true;
    }
}

/// Reads one `#[from(...)]`, appending an entry per type listed in it.
fn parse_from_attribute(attr: &Attribute, ast: &mut Ast, errors: &mut Errors) {
    if !matches!(attr.meta, Meta::List(_)) {
        errors.add(
            &attr.meta,
            "`#[from(...)]` takes a parenthesized list of types, such as `#[from(std::io::Error)]`",
        );
        return;
    }

    match attr.parse_args_with(Punctuated::<FromEntry, Token![,]>::parse_terminated) {
        Ok(entries) if entries.is_empty() => errors.add(
            &attr.meta,
            "`#[from(...)]` needs at least one type, such as `#[from(std::io::Error)]`",
        ),
        Ok(entries) => ast.conversions.extend(entries.into_iter().map(|entry| entry.0)),
        Err(error) => errors.combine(error),
    }
}

/// One entry of a `#[from(...)]` list: a type, optionally followed by field expressions.
struct FromEntry(FromAttr);

impl Parse for FromEntry {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let source = source_type(input)?;

        let overrides = if input.peek(syn::token::Paren) {
            let content;
            _ = syn::parenthesized!(content in input);
            Punctuated::<FromOverrideSyntax, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .map(|entry| entry.0)
                .collect()
        } else {
            Vec::new()
        };

        Ok(Self(FromAttr { source, overrides }))
    }
}

/// The source type of one `#[from(...)]` entry.
///
/// `syn` decides where the type ends, so a generic argument list keeps its commas, a type macro
/// keeps its parentheses, and a following `(member: expression)` override list is left for the
/// caller. Only an entry that opens with that override list has to be rejected here, because a
/// type is what an entry has to start with.
fn source_type(input: ParseStream<'_>) -> syn::Result<Type> {
    if holds_overrides(input) {
        return Err(input.error("expected a type"));
    }

    input.parse::<Type>()
}

/// Whether a parenthesis group opens at `input` and holds field overrides rather than type syntax.
///
/// An override list opens with a member and a `:`. The `:` has to stand alone, so the `std` of a
/// parenthesized `(std::io::Error)` does not read as a member.
fn holds_overrides(input: ParseStream<'_>) -> bool {
    let Some((inner, ..)) = input.cursor().group(Delimiter::Parenthesis) else {
        return false;
    };

    let after_key = inner
        .ident()
        .map(|(_, rest)| rest)
        .or_else(|| inner.literal().map(|(_, rest)| rest));

    matches!(
        after_key.and_then(Cursor::punct),
        Some((punct, _)) if punct.as_char() == ':' && punct.spacing() == Spacing::Alone
    )
}

/// One `key: expression` pair inside a `#[from(...)]` entry.
struct FromOverrideSyntax(FromOverride);

impl Parse for FromOverrideSyntax {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let key = input.parse::<Member>()?;
        _ = input.parse::<Token![:]>()?;
        let value = input.parse::<Expr>()?;
        Ok(Self(FromOverride { key, value }))
    }
}

/// Reads the struct's fields, rejecting the inputs that cannot hold a core.
fn parse_fields(ident: &syn::Ident, data: Data, errors: &mut Errors) -> Option<(Style, Vec<AstField>)> {
    let fields = match data {
        Data::Struct(data) => data.fields,
        Data::Enum(_) => {
            errors.add(
                ident,
                "`#[derive(ohno::Error)]` supports structs only. An enum has no single field to hold the OhnoCore",
            );
            return None;
        }
        Data::Union(_) => {
            errors.add(ident, "`#[derive(ohno::Error)]` supports structs only");
            return None;
        }
    };

    let style = match &fields {
        Fields::Named(_) => Style::Named,
        Fields::Unnamed(_) => Style::Tuple,
        Fields::Unit => {
            errors.add(
                ident,
                "`#[derive(ohno::Error)]` needs a field to hold the OhnoCore, and a unit struct has none. \
                 Declare one, or use `#[ohno::error]`, which adds it",
            );
            return None;
        }
    };

    let parsed = fields
        .into_iter()
        .enumerate()
        .map(|(index, field)| {
            let member = field
                .ident
                .clone()
                .map_or_else(|| Member::Unnamed(Index::from(index)), Member::Named);

            let mut marks = Vec::new();
            let mut generated = false;
            for attr in field.attrs {
                if attr.path().is_ident("error") {
                    check_bare_marker(&attr, "error", errors);
                    marks.push(attr);
                } else if marker::is_generated_marker(&attr) {
                    generated = true;
                }
            }

            AstField {
                member,
                ty: field.ty,
                marks,
                generated,
            }
        })
        .collect();

    Some((style, parsed))
}

/// Reports a bare marker attribute that carries anything beyond the bare word.
///
/// The flag is kept either way: the user has said what they want, so ignoring the attribute here
/// would report the consequence of a missing item on top of the malformed attribute.
fn check_bare_marker(attr: &Attribute, name: &str, errors: &mut Errors) {
    if !matches!(attr.meta, Meta::Path(_)) {
        errors.add(&attr.meta, format!("`#[{name}]` takes no arguments"));
    }
}

/// Renders a member the way a diagnostic spells it: `path`, or `0`.
#[must_use]
pub(crate) fn member_name(member: &Member) -> String {
    match member {
        Member::Named(ident) => ident.to_string(),
        Member::Unnamed(index) => index.index.to_string(),
    }
}

/// Whether the last segment of the type's path is `OhnoCore`.
///
/// Nothing is resolved, so a core reached through a type alias or a renamed import is invisible
/// here and has to be marked.
#[must_use]
pub(crate) fn is_ohno_core(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };

    path.qself.is_none() && path.path.segments.last().is_some_and(|segment| segment.ident == "OhnoCore")
}
