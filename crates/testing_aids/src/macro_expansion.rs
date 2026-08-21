// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Helpers for snapshot-testing what a procedural macro expands to.
//!
//! For a procedural macro the generated code *is* the behaviour, so the natural test is to
//! snapshot the whole expansion rather than probe it with substring assertions. Raw
//! `TokenStream` text is unreadable in a diff, so an expansion is parsed back into a
//! [`syn::File`] and pretty-printed before it reaches the snapshot.
//!
//! Both helpers panic rather than degrade. An expansion that no longer parses is a defect in
//! the macro, and rendering it as raw tokens instead would hide that defect behind a snapshot
//! that merely looks reformatted.

use proc_macro2::TokenStream;

/// Parses Rust source text into a token stream, for feeding a macro entry point.
///
/// # Panics
///
/// Panics if `source` does not tokenize.
#[must_use]
pub fn tokenize(source: &str) -> TokenStream {
    source.parse().expect("the source tokenizes")
}

/// Renders a macro expansion as formatted Rust source, ready to snapshot.
///
/// # Panics
///
/// Panics if `tokens` is not a parsable Rust file. A macro that emits something else is
/// broken, so this is deliberately not a recoverable case.
#[must_use]
pub fn render_expansion(tokens: TokenStream) -> String {
    let raw = tokens.to_string();
    let file = syn::parse2(tokens).unwrap_or_else(|err| panic!("the expansion parses as a Rust file: {err}\n--- tokens ---\n{raw}"));
    prettyplease::unparse(&file)
}

/// Renders tokens as formatted Rust source, falling back to the raw token text when they do
/// not parse as a Rust file.
///
/// Use this only to render macro *input*. A test that feeds a macro something which is not an
/// item at all -- `1 + 1`, a bare expression, a fragment -- still wants to show what it fed.
///
/// Never use it for an expansion. An expansion that does not parse is a defect in the macro,
/// and this fallback would bury it in a snapshot that merely looks badly formatted. Use
/// [`render_expansion`] there, which fails loudly instead.
#[must_use]
pub fn render_tokens_lossy(tokens: &TokenStream) -> String {
    syn::parse2(tokens.clone()).map_or_else(|_| tokens.to_string(), |file| prettyplease::unparse(&file))
}
