// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `DeriveInput` into [`Ast`].
//!
//! This phase answers one question per attribute: can it be read? An attribute that cannot is
//! reported here and left out of the `Ast`, so validation has nothing to check for it and reports
//! only faults it can actually see.

use proc_macro2::{Delimiter, Spacing, TokenTree};
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
        // The type is collected token by token rather than handed straight to `Type::parse`,
        // because a following `(kind: ...)` group is not a parenthesized generic argument list and
        // would make that parse fail. A parenthesis group that holds no `key: value` pair is part
        // of the type instead — a tuple, or a function's parameter list — so it is collected.
        let mut tokens = proc_macro2::TokenStream::new();
        while !input.is_empty() && !input.peek(Token![,]) && !holds_overrides(input) {
            tokens.extend(std::iter::once(input.parse::<TokenTree>()?));
        }

        if tokens.is_empty() {
            return Err(input.error("expected a type"));
        }

        let source = syn::parse2::<Type>(tokens)?;

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
pub(crate) fn is_ohno_core(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };

    path.qself.is_none() && path.path.segments.last().is_some_and(|segment| segment.ident == "OhnoCore")
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn parse_ok(input: DeriveInput) -> Ast {
        let mut errors = Errors::default();
        let ast = parse(input, &mut errors).expect("the input parses");
        assert!(errors.is_empty(), "unexpected faults: {}", errors.into_compile_error());
        ast
    }

    fn parse_faults(input: DeriveInput) -> String {
        let mut errors = Errors::default();
        _ = parse(input, &mut errors);
        errors.into_compile_error().to_string()
    }

    #[test]
    fn reads_a_named_struct() {
        let ast = parse_ok(parse_quote! {
            struct T {
                path: String,
                #[error]
                inner: ohno::OhnoCore,
            }
        });

        assert_eq!(ast.style, Style::Named);
        assert_eq!(ast.fields.len(), 2);
        assert_eq!(member_name(&ast.fields[0].member), "path");
        assert!(ast.fields[0].marks.is_empty());
        assert_eq!(ast.fields[1].marks.len(), 1);
        assert!(ast.fields.iter().all(AstField::is_referenceable));
    }

    #[test]
    fn reads_a_tuple_struct_by_index() {
        let ast = parse_ok(parse_quote!(
            struct T(String, #[error] ohno::OhnoCore);
        ));

        assert_eq!(ast.style, Style::Tuple);
        assert_eq!(member_name(&ast.fields[0].member), "0");
        assert_eq!(member_name(&ast.fields[1].member), "1");
    }

    #[test]
    fn recognizes_the_generated_marker() {
        let ast = parse_ok(parse_quote! {
            struct T {
                path: String,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                ohno_core: ohno::OhnoCore,
            }
        });

        assert!(ast.fields[0].is_referenceable());
        assert!(!ast.fields[1].is_referenceable());
    }

    #[test]
    fn reads_the_display_attribute() {
        let ast = parse_ok(parse_quote! {
            #[display("failed for {path}: {}", code.value())]
            struct T {
                path: String,
                code: u32,
                inner: ohno::OhnoCore,
            }
        });

        let display = ast.display.expect("the display attribute is read");
        assert_eq!(display.template.value(), "failed for {path}: {}");
        assert_eq!(display.arguments.len(), 1);
    }

    #[test]
    fn reads_the_flags() {
        let ast = parse_ok(parse_quote! {
            #[no_debug]
            #[no_constructors]
            struct T {
                inner: ohno::OhnoCore,
            }
        });

        assert!(ast.no_debug);
        assert!(ast.no_constructors);
    }

    #[test]
    fn reads_several_from_attributes_and_their_overrides() {
        let ast = parse_ok(parse_quote! {
            #[from(std::io::Error(kind: error.kind()), std::fmt::Error)]
            #[from(std::num::ParseIntError)]
            struct T {
                kind: std::io::ErrorKind,
                inner: ohno::OhnoCore,
            }
        });

        assert_eq!(ast.conversions.len(), 3);
        assert_eq!(ast.conversions[0].overrides.len(), 1);
        assert_eq!(member_name(&ast.conversions[0].overrides[0].key), "kind");
        assert!(ast.conversions[1].overrides.is_empty());
    }

    #[test]
    fn reads_a_tuple_keyed_from_override() {
        let ast = parse_ok(parse_quote! {
            #[from(std::io::Error(0: error.kind()))]
            struct T(std::io::ErrorKind, ohno::OhnoCore);
        });

        assert_eq!(member_name(&ast.conversions[0].overrides[0].key), "0");
    }

    #[test]
    fn rejects_an_enum() {
        assert!(
            parse_faults(parse_quote!(
                enum T {
                    A,
                }
            ))
            .contains("structs only")
        );
    }

    #[test]
    fn rejects_a_union() {
        assert!(parse_faults(parse_quote!(union T { a: u32 })).contains("structs only"));
    }

    #[test]
    fn rejects_a_unit_struct() {
        assert!(
            parse_faults(parse_quote!(
                struct T;
            ))
            .contains("unit struct has none")
        );
    }

    #[test]
    fn rejects_a_marker_with_arguments_but_keeps_the_mark() {
        let mut errors = Errors::default();
        let ast = parse(
            parse_quote! {
                struct T {
                    #[error(nonsense)]
                    inner: Core,
                }
            },
            &mut errors,
        )
        .expect("the struct still parses");

        assert!(errors.into_compile_error().to_string().contains("takes no arguments"));
        assert_eq!(ast.fields[0].marks.len(), 1);
    }

    #[test]
    fn rejects_every_malformed_marker_shape() {
        for input in [
            parse_quote! { struct T { #[error()] inner: Core, } },
            parse_quote! { struct T { #[error = "x"] inner: Core, } },
        ] {
            assert!(parse_faults(input).contains("takes no arguments"));
        }
    }

    #[test]
    fn rejects_a_malformed_from_attribute() {
        for input in [
            parse_quote! { #[from] struct T { inner: ohno::OhnoCore, } },
            parse_quote! { #[from = "x"] struct T { inner: ohno::OhnoCore, } },
        ] {
            assert!(parse_faults(input).contains("parenthesized list of types"));
        }
    }

    #[test]
    fn rejects_an_empty_from_attribute() {
        let faults = parse_faults(parse_quote! { #[from()] struct T { inner: ohno::OhnoCore, } });
        assert!(faults.contains("at least one type"), "{faults}");
    }

    #[test]
    fn rejects_a_from_entry_that_names_no_type() {
        let faults = parse_faults(parse_quote! { #[from((kind: 1))] struct T { inner: ohno::OhnoCore, } });
        assert!(faults.contains("expected a type"), "{faults}");
    }

    #[test]
    fn reads_a_source_type_written_with_parentheses() {
        // Parentheses appear inside types too, so a group is only read as an override list when it
        // holds `key: value` pairs.
        for input in [
            parse_quote! { #[from((u32, String))] struct T { inner: ohno::OhnoCore, } },
            parse_quote! { #[from(fn() -> std::io::Error)] struct T { inner: ohno::OhnoCore, } },
            parse_quote! { #[from(Box<dyn Fn() -> u8>)] struct T { inner: ohno::OhnoCore, } },
        ] {
            let mut errors = Errors::default();
            let ast = parse(input, &mut errors).expect("the input parses");
            assert!(errors.is_empty(), "{}", errors.into_compile_error());
            assert_eq!(ast.conversions.len(), 1);
        }
    }

    #[test]
    fn rejects_a_flag_that_carries_arguments() {
        for input in [
            parse_quote! { #[no_debug(foo)] struct T { inner: ohno::OhnoCore, } },
            parse_quote! { #[no_constructors = "x"] struct T { inner: ohno::OhnoCore, } },
        ] {
            assert!(parse_faults(input).contains("takes no arguments"));
        }
    }

    #[test]
    fn rejects_a_malformed_display_attribute() {
        let faults = parse_faults(parse_quote! { #[display] struct T { inner: ohno::OhnoCore, } });
        assert!(!faults.is_empty());
    }

    #[test]
    fn recognizes_an_ohno_core_type_by_its_last_segment() {
        assert!(is_ohno_core(&parse_quote!(OhnoCore)));
        assert!(is_ohno_core(&parse_quote!(ohno::OhnoCore)));
        assert!(!is_ohno_core(&parse_quote!(Core)));
        assert!(!is_ohno_core(&parse_quote!(&'a str)));
        assert!(!is_ohno_core(&parse_quote!(<T as Trait>::OhnoCore)));
    }
}
