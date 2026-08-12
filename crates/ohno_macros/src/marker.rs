// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The reserved marker that identifies the `OhnoCore` field `#[ohno::error]` adds.
//!
//! A derive helper attribute has to be listed in `attributes(...)` to be inert, and everything in
//! that list appears in the derive's public rustdoc. A doc comment needs no such listing, and the
//! added field is private, so the marker stays out of the docs.
//!
//! The cost falls on the derive, which runs after `#[ohno::error]` has added the field and so can
//! only compare text. That is why the marker ends in a nonce: an ordinary doc comment does not
//! match it.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Expr, Lit, Meta};

/// The text of the reserved doc comment, without the leading space a `///` comment carries.
pub(crate) const GENERATED_ERROR_FIELD_MARKER: &str = "ohno::generated-core@7f3d9c2a";

/// The attribute `#[ohno::error]` puts on the field it adds.
///
/// Written in the shape a `///` comment produces, so a hand-written copy and a generated one are
/// the same tokens.
pub(crate) fn generated_marker() -> TokenStream {
    let text = format!(" {GENERATED_ERROR_FIELD_MARKER}");
    quote!(#[doc = #text])
}

/// Returns `true` when `attr` is the reserved marker.
///
/// The comparison is trimmed, so it recognizes the marker whether it was written as a `///`
/// comment (which carries a leading space) or as a bare `#[doc = "..."]`.
pub(crate) fn is_generated_marker(attr: &Attribute) -> bool {
    let Meta::NameValue(name_value) = &attr.meta else {
        return false;
    };

    if !name_value.path.is_ident("doc") {
        return false;
    }

    let Expr::Lit(literal) = &name_value.value else {
        return false;
    };

    let Lit::Str(text) = &literal.lit else {
        return false;
    };

    text.value().trim() == GENERATED_ERROR_FIELD_MARKER
}

#[cfg(test)]
mod tests {
    use syn::parse::Parser as _;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn recognizes_the_marker_it_writes() {
        let attrs = Attribute::parse_outer
            .parse2(generated_marker())
            .expect("the generated marker parses");
        assert!(attrs.iter().any(is_generated_marker));
    }

    #[test]
    fn recognizes_a_hand_written_doc_comment_carrying_the_marker() {
        let attr: Attribute = parse_quote!(#[doc = " ohno::generated-core@7f3d9c2a"]);
        assert!(is_generated_marker(&attr));
    }

    #[test]
    fn an_ordinary_doc_comment_is_not_the_marker() {
        let attr: Attribute = parse_quote!(#[doc = " Where the failure happened."]);
        assert!(!is_generated_marker(&attr));
    }

    #[test]
    fn a_doc_comment_mentioning_the_marker_is_not_the_marker() {
        let attr: Attribute = parse_quote!(#[doc = " The ohno generated core field is not this one."]);
        assert!(!is_generated_marker(&attr));
    }

    #[test]
    fn a_non_doc_attribute_is_not_the_marker() {
        let attr: Attribute = parse_quote!(#[error]);
        assert!(!is_generated_marker(&attr));

        let attr: Attribute = parse_quote!(#[doc(hidden)]);
        assert!(!is_generated_marker(&attr));

        let attr: Attribute = parse_quote!(#[doc = 1]);
        assert!(!is_generated_marker(&attr));

        let attr: Attribute = parse_quote!(#[doc = concat!(" ohno::generated-core@7f3d9c2a")]);
        assert!(!is_generated_marker(&attr));

        let attr: Attribute = parse_quote!(#[other = " ohno::generated-core@7f3d9c2a"]);
        assert!(!is_generated_marker(&attr));
    }
}
