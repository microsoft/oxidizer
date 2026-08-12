// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[ohno::error]`.
//!
//! The attribute rewrites a struct so that it holds an `OhnoCore`, then applies
//! `#[derive(ohno::Error)]` to the result. It keeps no model of its own: the derive validates the
//! rewritten struct again, so a second one would only restate what the derive already holds.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::Parser as _;
use syn::{Field, Fields, FieldsNamed, FieldsUnnamed, Item, ItemStruct};

use crate::diagnostics::Errors;
use crate::marker;

/// The diagnostic for a field marked with `#[error]`.
const ALREADY_MARKED: &str = "`#[ohno::error]` adds the OhnoCore field itself and generates the error representation from it, \
     so no field may be marked with `#[error]`. Remove the marker to keep the field as data, or use \
     `#[derive(ohno::Error)]` to place the core explicitly";

/// The diagnostic for a hand-written copy of the reserved marker.
const RESERVED_MARKER: &str = "This doc comment is reserved for `#[ohno::error]`, which puts it on the OhnoCore field it adds. \
     Remove it; if this is the field holding the OhnoCore, use `#[derive(ohno::Error)]` and mark it \
     with `#[error]`";

/// The diagnostic for `#[no_constructors]`.
const NO_CONSTRUCTORS: &str = "`#[no_constructors]` is not supported under `#[ohno::error]`. A constructor has to initialize the \
     OhnoCore field, and the field inserted by `#[ohno::error]` is an implementation detail with no \
     stable name or position, so it must not be referred to in code. Use `#[derive(ohno::Error)]` and \
     declare the OhnoCore field explicitly";

/// The base name of the field the attribute adds.
const CORE_FIELD_NAME: &str = "ohno_core";

/// Expands the attribute, or renders everything that stopped it.
pub(crate) fn expand(item: Item) -> TokenStream {
    let mut errors = Errors::default();

    let Item::Struct(mut item) = item else {
        errors.add(
            &item,
            "`#[ohno::error]` supports structs only. A struct is what can hold the OhnoCore field it adds",
        );
        return errors.into_compile_error();
    };

    reject(&item, &mut errors);
    if !errors.is_empty() {
        return errors.into_compile_error();
    }

    inject_core(&mut item);

    quote! {
        #[derive(::ohno::Error)]
        #item
    }
}

/// Reports everything the attribute cannot rewrite.
///
/// All three checks run before anything is added, which is what lets the attribute say a reserved
/// marker was hand-written. The derive runs after and can only compare text, so by then the two are
/// the same.
fn reject(item: &ItemStruct, errors: &mut Errors) {
    for attr in &item.attrs {
        if attr.path().is_ident("no_constructors") {
            errors.add(attr, NO_CONSTRUCTORS);
        }
    }

    for field in &item.fields {
        for attr in &field.attrs {
            if attr.path().is_ident("error") {
                errors.add(attr, ALREADY_MARKED);
            }
        }

        if field.attrs.iter().any(marker::is_generated_marker) {
            errors.add(field, RESERVED_MARKER);
        }
    }
}

/// Adds the `OhnoCore` field, carrying the reserved marker.
///
/// A unit struct becomes a tuple struct holding the field, which is what gives it room for a core.
fn inject_core(item: &mut ItemStruct) {
    let marker = marker::generated_marker();

    match &mut item.fields {
        Fields::Named(fields) => {
            let ident = format_ident!("{}", unused_name(fields));
            let field = Field::parse_named
                .parse2(quote!(#marker #ident: ::ohno::OhnoCore))
                .expect("the added named field parses");
            fields.named.push(field);
        }
        Fields::Unnamed(fields) => fields.unnamed.push(unnamed_core(&marker)),
        Fields::Unit => {
            let mut unnamed = FieldsUnnamed {
                paren_token: syn::token::Paren::default(),
                unnamed: syn::punctuated::Punctuated::new(),
            };
            unnamed.unnamed.push(unnamed_core(&marker));
            item.fields = Fields::Unnamed(unnamed);
            item.semi_token = Some(<syn::Token![;]>::default());
        }
    }
}

/// The core as a positional field.
fn unnamed_core(marker: &TokenStream) -> Field {
    Field::parse_unnamed
        .parse2(quote!(#marker ::ohno::OhnoCore))
        .expect("the added positional field parses")
}

/// A name for the added field that no declared field already uses.
///
/// The search is bounded by the field count plus one, which is more candidates than there are
/// fields, so one of them is always free.
fn unused_name(fields: &FieldsNamed) -> String {
    let taken = |candidate: &str| {
        fields
            .named
            .iter()
            .any(|field| field.ident.as_ref().is_some_and(|i| i == candidate))
    };

    if !taken(CORE_FIELD_NAME) {
        return CORE_FIELD_NAME.to_owned();
    }

    (1..=fields.named.len())
        .map(|n| format!("{CORE_FIELD_NAME}_{n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    /// Expands the attribute and pretty-prints the result.
    ///
    /// The whole rewritten struct is snapshotted rather than searched for substrings, because what
    /// the attribute has to get right is the struct it hands to the derive: where the added field
    /// lands, what it is named, and what of the original survives beside it.
    fn rendered(item: Item) -> String {
        let expanded = expand(item);
        let file: syn::File = syn::parse2(expanded).expect("the expansion parses as a file");
        prettyplease::unparse(&file)
    }

    #[test]
    fn a_named_struct_gains_a_named_core() {
        insta::assert_snapshot!(rendered(parse_quote! {
            struct T { path: String }
        }));
    }

    #[test]
    fn a_colliding_name_is_numbered() {
        insta::assert_snapshot!(rendered(parse_quote! {
            struct T { ohno_core: u32, ohno_core_1: u32 }
        }));
    }

    #[test]
    fn a_tuple_struct_gains_a_trailing_core() {
        insta::assert_snapshot!(rendered(parse_quote!(
            struct T(String);
        )));
    }

    #[test]
    fn a_unit_struct_becomes_a_tuple_struct() {
        insta::assert_snapshot!(rendered(parse_quote!(
            struct T;
        )));
    }

    #[test]
    fn other_attributes_and_docs_survive() {
        insta::assert_snapshot!(rendered(parse_quote! {
            /// Documentation for T.
            #[derive(Clone)]
            #[display("failed for {path}")]
            struct T { path: String }
        }));
    }

    #[test]
    fn an_ordinary_doc_comment_is_left_alone() {
        insta::assert_snapshot!(rendered(parse_quote! {
            struct T {
                /// Where the failure happened.
                path: String,
            }
        }));
    }

    #[test]
    fn a_marked_field_is_rejected() {
        insta::assert_snapshot!(rendered(parse_quote! {
            struct T { path: String, #[error] mine: ohno::OhnoCore }
        }));
    }

    #[test]
    fn a_hand_written_reserved_marker_is_rejected() {
        insta::assert_snapshot!(rendered(parse_quote! {
            struct T {
                path: String,
                #[doc = " ohno::generated-core@7f3d9c2a"]
                mine: ohno::OhnoCore,
            }
        }));
    }

    #[test]
    fn no_constructors_is_rejected() {
        insta::assert_snapshot!(rendered(parse_quote! {
            #[no_constructors]
            struct T { path: String }
        }));
    }

    #[test]
    fn a_non_struct_is_rejected() {
        insta::assert_snapshot!(rendered(parse_quote!(
            enum T {
                A,
            }
        )));
    }

    #[test]
    fn a_rejected_struct_is_not_rewritten() {
        // A rejection stops the rewrite entirely, so nothing reaches the derive. Emitting a
        // half-rewritten struct beside the diagnostic would make rustc report the consequences of
        // the fault as well as the fault itself.
        let expanded = expand(parse_quote! {
            struct T { path: String, #[error] mine: ohno::OhnoCore }
        })
        .to_string();

        assert!(!expanded.contains("struct T"), "{expanded}");
    }
}
