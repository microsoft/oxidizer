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
#[must_use]
pub(crate) fn generated_marker() -> TokenStream {
    let text = format!(" {GENERATED_ERROR_FIELD_MARKER}");
    quote!(#[doc = #text])
}

/// Returns `true` when `attr` is the reserved marker.
///
/// The comparison is trimmed, so it recognizes the marker whether it was written as a `///`
/// comment (which carries a leading space) or as a bare `#[doc = "..."]`.
#[must_use]
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
