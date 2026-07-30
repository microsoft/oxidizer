// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashSet;

use proc_macro2::TokenStream as TokenStream2;
use quote::ToTokens;
use syn::DeriveInput;

/// Collects every identifier appearing in `tokens` (recursively descending into groups) into
/// `out`, so generated helper names can be made collision-free against the input.
///
/// String literals are also inspected: serde routes custom code through
/// string-valued paths (e.g. `#[serde(with = "path::to::fn")]`), so any literal
/// that parses as a path contributes its segment identifiers too. That keeps a
/// user-provided `with` / `deserialize_with` / `serialize_with` function from
/// colliding with a generated helper unit-struct name.
pub(crate) fn collect_ident_strings(tokens: TokenStream2, out: &mut HashSet<String>) {
    for token in tokens {
        match token {
            proc_macro2::TokenTree::Ident(ident) => {
                out.insert(ident.to_string());
            }
            proc_macro2::TokenTree::Group(group) => collect_ident_strings(group.stream(), out),
            proc_macro2::TokenTree::Literal(literal) => {
                if let Ok(text) = syn::parse2::<syn::LitStr>(literal.to_token_stream())
                    && let Ok(path) = syn::parse_str::<syn::Path>(&text.value())
                {
                    for segment in path.segments {
                        out.insert(segment.ident.to_string());
                    }
                }
            }
            proc_macro2::TokenTree::Punct(_) => {}
        }
    }
}

/// The set of identifier spellings used anywhere in the derive input.
pub(crate) fn used_identifiers(input: &DeriveInput) -> HashSet<String> {
    let mut set = HashSet::new();
    collect_ident_strings(input.to_token_stream(), &mut set);
    set
}

/// Produces a generated helper identifier that cannot collide with any name in
/// the input (appending `_` until unique) and carries `Span::mixed_site()`
/// hygiene, so a caller cannot shadow or capture it. Mirrors the approach in
/// `multitude_macros_impl`.
pub(crate) fn fresh_ident(used: &HashSet<String>, base: &str) -> syn::Ident {
    let mut candidate = base.to_owned();
    while used.contains(&candidate) {
        candidate.push('_');
    }
    syn::Ident::new(&candidate, proc_macro2::Span::mixed_site())
}
