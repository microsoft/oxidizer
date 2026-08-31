// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Helpers for snapshot-testing what a procedural macro expands to.
//!
//! For a procedural macro the generated code *is* the behaviour, so the natural test is to
//! snapshot the whole expansion rather than probe it with substring assertions. Raw
//! `TokenStream` text is unreadable in a diff, so an expansion is parsed back into a
//! [`syn::File`] and pretty-printed before it reaches the snapshot.
//!
//! [`tokenize`] and [`render_expansion`] panic rather than degrade: an expansion that no longer
//! parses is a defect in the macro, and rendering it as raw tokens instead would hide that
//! defect behind a snapshot that merely looks reformatted. [`render_tokens_lossy`] is the one
//! exception, and it is for macro *input* only -- a test that deliberately feeds a macro
//! something which is not an item still wants to show what it fed.

use proc_macro2::TokenStream;

/// Parses Rust source text into a token stream, for feeding a macro entry point.
///
/// # Panics
///
/// Panics if `source` is not valid Rust tokens.
#[must_use]
pub fn tokenize(source: &str) -> TokenStream {
    source
        .parse()
        .unwrap_or_else(|err| panic!("the source tokenizes as Rust: {err}\n--- source ---\n{source}"))
}

/// Renders a macro expansion as formatted Rust source, ready to snapshot.
///
/// # Panics
///
/// Panics if `tokens` do not parse as a Rust file. A macro that emits something else is
/// broken, so this is deliberately not a recoverable case.
#[must_use]
pub fn render_expansion(tokens: &TokenStream) -> String {
    // Taken by reference, like `render_tokens_lossy`: `syn::parse2` needs an owned stream, so
    // the parse clones (which is cheap), and rendering the tokens stays inside the panic arm
    // rather than running on the successful path that no snapshot suite ever reads.
    let file =
        syn::parse2(tokens.clone()).unwrap_or_else(|err| panic!("the expansion parses as a Rust file: {err}\n--- tokens ---\n{tokens}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokens that are an expression, not an item, so they cannot form a Rust file.
    fn not_a_file() -> TokenStream {
        tokenize("1 + 1")
    }

    #[test]
    fn an_expansion_is_rendered_as_formatted_source() {
        let rendered = render_expansion(&tokenize("struct S { a: u8 }"));

        assert_eq!(rendered, "struct S {\n    a: u8,\n}\n");
    }

    #[test]
    #[should_panic(expected = "the expansion parses as a Rust file")]
    fn an_expansion_that_is_not_a_file_fails_the_test() {
        // The point of the strict helper: a macro that emits unparsable tokens must fail
        // here rather than reach a snapshot as raw token text, where it would look like a
        // formatting change and be refreshed away.
        _ = render_expansion(&not_a_file());
    }

    #[test]
    fn the_panic_message_carries_the_offending_tokens() {
        let panic = std::panic::catch_unwind(|| render_expansion(&not_a_file())).expect_err("rendering a non-file panics");
        let message = panic.downcast_ref::<String>().expect("the panic payload is a formatted string");

        assert!(message.contains("1 + 1"), "the tokens are reported: {message}");
    }

    #[test]
    fn lossy_rendering_falls_back_to_the_raw_tokens() {
        assert_eq!(render_tokens_lossy(&not_a_file()), "1 + 1");
    }

    #[test]
    fn lossy_rendering_still_formats_what_does_parse() {
        assert_eq!(render_tokens_lossy(&tokenize("struct S { a: u8 }")), "struct S {\n    a: u8,\n}\n");
    }
}
