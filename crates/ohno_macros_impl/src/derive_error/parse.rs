// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `DeriveInput` into [`Ast`].
//!
//! This phase answers one question per attribute: can it be read? An attribute that cannot is
//! reported here and left out of the `Ast`, so validation has nothing to check for it and reports
//! only faults it can actually see.

use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Attribute, Data, DeriveInput, Expr, Field, Fields, Index, Member, Meta, Token, Type, token};

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
///
/// An attribute the crate does not own is left alone, whatever it says.
fn parse_struct_attribute(attr: &Attribute, ast: &mut Ast, errors: &mut Errors) {
    let Some(name) = attr.path().get_ident() else {
        return;
    };

    match name.to_string().as_str() {
        "display" => parse_display_attribute(attr, ast, errors),
        "from" => parse_from_attribute(attr, ast, errors),
        "no_debug" => {
            check_bare_marker(attr, "no_debug", errors);
            ast.no_debug = true;
        }
        "no_constructors" => {
            check_bare_marker(attr, "no_constructors", errors);
            ast.no_constructors = true;
        }
        _ => {}
    }
}

/// Reads one `#[display(...)]`, of which there may be only one.
fn parse_display_attribute(attr: &Attribute, ast: &mut Ast, errors: &mut Errors) {
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

    match attr.parse_args_with(Punctuated::<FromAttr, Token![,]>::parse_terminated) {
        Ok(entries) if entries.is_empty() => errors.add(
            &attr.meta,
            "`#[from(...)]` needs at least one type, such as `#[from(std::io::Error)]`",
        ),
        Ok(entries) => ast.conversions.extend(entries),
        Err(error) => errors.combine(error),
    }
}

impl Parse for FromAttr {
    /// One entry of a `#[from(...)]` list: a type, optionally followed by field overrides.
    ///
    /// `syn` decides where the type ends, so a generic argument list keeps its commas and a type
    /// macro keeps its parentheses. Only an entry that *opens* with an override list has to be
    /// rejected here, because a type is what an entry has to start with.
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if opens_overrides(input) {
            return Err(input.error("expected a type"));
        }

        let source = input.parse::<Type>()?;
        let overrides = if input.peek(token::Paren) {
            input.parse::<Overrides>()?.0
        } else {
            Vec::new()
        };

        Ok(Self { source, overrides })
    }
}

/// Whether an override list, rather than a parenthesized type, opens at `input`.
///
/// A parenthesized type such as `(std::io::Error)` opens with the same token, so the contents
/// decide. Parsing them on a fork is what asks that question: an override list is exactly what
/// [`Overrides`] accepts, and `(std::io::Error)` is not, because the `:` of a path leaves an
/// expression that cannot be parsed behind it.
///
/// An *empty* list is excluded, so `()` reads as the unit type. It is a type an entry may not use,
/// but that is a rule, and [`validate`](super::validate) reports it against the `()` itself. Read
/// here as an empty override list instead, it would fail as "expected a type" — and because the
/// entries of one attribute parse as a single list, that failure would take every other entry in
/// the attribute with it.
fn opens_overrides(input: ParseStream<'_>) -> bool {
    input.peek(token::Paren) && input.fork().parse::<Overrides>().is_ok_and(|overrides| !overrides.0.is_empty())
}

/// The parenthesized `member: expression` list a `#[from(...)]` entry may end with.
struct Overrides(Vec<FromOverride>);

impl Parse for Overrides {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let content;
        _ = syn::parenthesized!(content in input);

        Ok(Self(
            Punctuated::<FromOverride, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .collect(),
        ))
    }
}

impl Parse for FromOverride {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let key = input.parse::<Member>()?;
        _ = input.parse::<Token![:]>()?;
        let value = input.parse::<Expr>()?;
        Ok(Self { key, value })
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
        .map(|(index, field)| parse_field(index, field, errors))
        .collect();

    Some((style, parsed))
}

/// Reads one field, keeping every marker that may designate it as the error field.
fn parse_field(index: usize, field: Field, errors: &mut Errors) -> AstField {
    let Field { attrs, ident, ty, .. } = field;
    let member = ident.map_or_else(|| Member::Unnamed(Index::from(index)), Member::Named);

    let mut marks = Vec::new();
    let mut generated = false;

    for attr in attrs {
        if attr.path().is_ident("error") {
            check_bare_marker(&attr, "error", errors);
            marks.push(attr);
        } else if marker::is_generated_marker(&attr) {
            generated = true;
        }
    }

    AstField {
        member,
        ty,
        marks,
        generated,
    }
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
