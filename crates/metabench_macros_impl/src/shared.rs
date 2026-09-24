// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use proc_macro_crate::FoundCrate;
use proc_macro2::{Span, TokenStream, TokenTree};
use quote::{format_ident, quote};
use syn::{Error, Expr, ExprPath, Ident, Result};

pub(super) fn metabench_path() -> Result<TokenStream> {
    proc_macro_crate::crate_name("metabench")
        .map(|found| match found {
            FoundCrate::Itself => quote!(::metabench),
            FoundCrate::Name(name) => {
                let name = format_ident!("{name}");
                quote!(::#name)
            }
        })
        .map_err(|error| Error::new(Span::call_site(), error))
}

pub(super) fn parse_ident_value(value: &Expr, field: &str) -> Result<Ident> {
    let Expr::Path(ExprPath { path, .. }) = value else {
        return Err(Error::new_spanned(value, format!("`{field}` must be a Rust identifier")));
    };
    if path.leading_colon.is_some() || path.segments.len() != 1 {
        return Err(Error::new_spanned(path, format!("`{field}` must be a single Rust identifier")));
    }

    path.get_ident()
        .cloned()
        .ok_or_else(|| Error::new_spanned(path, format!("`{field}` must be a Rust identifier")))
}

/// Upper bound on the token-group nesting depth a single macro invocation's
/// raw tokens may reach: the number of currently-open token groups (parens,
/// brackets, braces). `syn`'s recursive-descent `Expr` parser recurses once
/// per nested group, so this bounds deeply nested parentheses/brackets/
/// braces (`(((...)))`). It deliberately does *not* bound the total number
/// of groups in an invocation, since ordinary breadth (many sibling groups
/// closed and reopened at the same nesting level — many benchmarks, many
/// identities) does not add recursion depth. 32 mirrors the safety margin
/// already used for `MAX_INPUT_LEN` below and is far beyond any legitimate
/// use in this workspace, where nesting rarely exceeds a handful of levels.
const MAX_GROUP_DEPTH: usize = 32;

/// Upper bound on the number of tokens allowed between two top-level
/// comma/semicolon separators within a single open group (or before the
/// first/after the last separator). `syn` never recurses *across* such a
/// separator — every `Punctuated` list and every statement sequence parses
/// each comma/semicolon-delimited item as an independent unit — so this
/// bounds every token shape that can recurse *within* one item without an
/// intervening group: prefix operators (`!!!!...x`), `return`/`break`/
/// `yield`/`become` chains, right-associative assignment chains
/// (`a = b = c = ...`), and any other construct `syn` parses recursively
/// now or in the future, without this guard needing to enumerate `syn`'s
/// grammar productions. A single hand-written field value (an identifier,
/// a literal, a short closure, a simple call expression, ...) never
/// approaches this budget, while breadth (many small fields, many
/// benchmarks, many identities, each separated by a comma) is completely
/// unrestricted, since every comma resets the count.
const MAX_FIELD_TOKENS: usize = 128;

/// Rejects `tokens` if walking it would ever reach a group nesting depth
/// beyond [`MAX_GROUP_DEPTH`] or a per-field token count beyond
/// [`MAX_FIELD_TOKENS`], before any recursive-descent parsing of the
/// tokens is attempted. The walk itself is iterative (an explicit
/// work-stack takes the place of recursive descent into nested groups) so
/// it cannot itself overflow the stack, regardless of how the input is
/// shaped.
pub(super) fn reject_if_too_complex(tokens: &TokenStream) -> Result<()> {
    // Every stack frame tracks the token iterator for one open group, plus
    // the running token count since the last top-level comma/semicolon
    // separator seen in that group (reset at each separator; a count only
    // accumulates recursion risk while it stays within one undivided item).
    let mut frames = vec![(tokens.clone().into_iter(), 0_usize)];

    while let Some((current, _)) = frames.last_mut() {
        let Some(token) = current.next() else {
            frames.pop();
            continue;
        };
        let group_depth = frames.len();
        let field_tokens = &mut frames.last_mut().expect("just matched Some above").1;

        if matches!(&token, TokenTree::Punct(punct) if matches!(punct.as_char(), ',' | ';')) {
            *field_tokens = 0;
            continue;
        }

        *field_tokens += 1;
        if *field_tokens > MAX_FIELD_TOKENS {
            return Err(too_complex_error(&token));
        }

        if let TokenTree::Group(group) = &token {
            if group_depth + 1 > MAX_GROUP_DEPTH {
                return Err(too_complex_error(&token));
            }
            frames.push((group.stream().into_iter(), 0));
        }
    }

    Ok(())
}

fn too_complex_error(token: &TokenTree) -> Error {
    Error::new(
        token.span(),
        "macro arguments are too complex (nested too deeply, or a single field's value has too many tokens); simplify the invocation",
    )
}

pub(super) fn support_ident(kind: &str, logical_name: &str) -> Ident {
    const OFFSET_BASIS: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

    let mut hash = OFFSET_BASIS;
    for byte in logical_name.bytes() {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format_ident!("__metabench_{kind}_{hash:032x}", span = Span::mixed_site())
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use proc_macro2::TokenStream;
    use quote::quote;

    use super::{MAX_FIELD_TOKENS, MAX_GROUP_DEPTH, parse_ident_value, reject_if_too_complex, support_ident};

    #[test]
    fn identifier_values_accept_only_one_unqualified_segment() {
        let accepted: syn::Expr = syn::parse_quote!(IDENTITY);
        assert_eq!(parse_ident_value(&accepted, "identity").unwrap(), "IDENTITY");

        for rejected in [
            syn::parse_quote!("identity"),
            syn::parse_quote!(module::IDENTITY),
            syn::parse_quote!(::IDENTITY),
            syn::parse_quote!(call()),
        ] {
            parse_ident_value(&rejected, "identity").unwrap_err();
        }
    }

    #[test]
    fn support_identifiers_are_stable_and_namespace_specific() {
        assert_eq!(support_ident("native", "IDENTITY"), support_ident("native", "IDENTITY"));
        assert_ne!(support_ident("native", "IDENTITY"), support_ident("adapter", "IDENTITY"));
        assert_ne!(support_ident("native", "IDENTITY"), support_ident("native", "OTHER"));
    }

    #[test]
    fn support_identifier_hash_matches_reference_fnv1a_128() {
        // Pins the exact hash so a mutation to the FNV-1a algorithm (e.g.
        // swapping `^=` for `|=` in the mixing step) is caught even though
        // it would still produce distinct, stable, fixed-length identifiers.
        assert_eq!(
            support_ident("target", "IDENTITY").to_string(),
            "__metabench_target_0838fce072659a7de8f7c5a0f94235a3"
        );
    }

    #[test]
    fn support_identifier_length_does_not_depend_on_logical_name_length() {
        let expected_length = "__metabench_target_".len() + 32;
        assert_eq!(support_ident("target", "x").to_string().len(), expected_length);
        assert_eq!(
            support_ident("target", &"identity:".repeat(1_000)).to_string().len(),
            expected_length
        );
    }

    #[test]
    fn overly_complex_argument_lists_are_rejected_before_parsing() {
        // Wide, but shallow, input is accepted regardless of its total
        // token count: every comma resets the per-field token budget, so
        // breadth (many small, comma-separated fields/benchmarks/
        // identities) never accumulates toward the limit.
        let wide: TokenStream = std::iter::repeat_n(quote!(x,), MAX_FIELD_TOKENS * 10).collect();
        reject_if_too_complex(&wide).unwrap();

        // A single field's value with a token count exactly at the budget
        // is accepted; one token past it is rejected — regardless of what
        // kind of token it is, since the guard counts tokens generically
        // rather than enumerating which `syn` constructs recurse.
        let at_limit: TokenStream = std::iter::repeat_n(quote!(!), MAX_FIELD_TOKENS).collect();
        reject_if_too_complex(&at_limit).unwrap();
        let over_limit: TokenStream = std::iter::repeat_n(quote!(!), MAX_FIELD_TOKENS + 1).collect();
        reject_if_too_complex(&over_limit).unwrap_err();

        // The group-nesting depth budget applies independently of the
        // per-field token budget.
        let mut nested = quote!(x);
        for _ in 0..MAX_GROUP_DEPTH {
            nested = quote!((#nested));
        }
        reject_if_too_complex(&nested).unwrap_err();

        // Many sibling groups at the same, shallow nesting level are fine.
        let mut siblings = TokenStream::new();
        for _ in 0..(MAX_GROUP_DEPTH * 10) {
            siblings.extend(quote!((x),));
        }
        reject_if_too_complex(&siblings).unwrap();

        // `return`/`break`/`yield`/`become` chains, and right-associative
        // assignment chains, all recurse in `syn` just like unary prefix
        // operators (each one recursively parses its own inner expression
        // without an intervening group), so a long-enough chain of any of
        // them is rejected by the same generic per-field token budget —
        // without this guard needing to enumerate `syn`'s grammar.
        let return_over_limit: TokenStream = std::iter::repeat_n(quote!(return), MAX_FIELD_TOKENS + 1).collect();
        reject_if_too_complex(&return_over_limit).unwrap_err();
        let break_over_limit: TokenStream = std::iter::repeat_n(quote!(break), MAX_FIELD_TOKENS + 1).collect();
        reject_if_too_complex(&break_over_limit).unwrap_err();
        let yield_over_limit: TokenStream = std::iter::repeat_n(quote!(yield), MAX_FIELD_TOKENS + 1).collect();
        reject_if_too_complex(&yield_over_limit).unwrap_err();
        let become_over_limit: TokenStream = std::iter::repeat_n(quote!(become), MAX_FIELD_TOKENS + 1).collect();
        reject_if_too_complex(&become_over_limit).unwrap_err();
        let assignment_over_limit: TokenStream = std::iter::repeat_n(quote!(a =), (MAX_FIELD_TOKENS + 1).div_ceil(2)).collect();
        reject_if_too_complex(&assignment_over_limit).unwrap_err();
    }

    // `syn::Expr`'s recursive-descent parser has no nesting-depth limit, so
    // deeply nested constructs (parens, brackets, or chains of unary/prefix
    // operators like `!!!!...x`) can exhaust the stack well before
    // `parse_ident_value` is ever reached. Unlike an already-parsed `Expr`,
    // proc-macro attribute arguments arrive from rustc as a raw, unparsed
    // token stream, so this really is reachable from real macro input, not
    // just this fuzz harness: `reject_if_too_complex` (below) is what
    // actually guards `benchmark`/`target::main`'s entry points against it.
    // Cap the input length here too, purely to keep this harness fuzzing
    // `parse_ident_value`'s own single-segment contract instead of `syn`'s
    // unrelated parser-depth characteristics.
    const MAX_INPUT_LEN: usize = 32;

    #[test]
    fn generated_identifier_inputs_obey_the_single_segment_contract() {
        bolero::check!().for_each(|input: &[u8]| {
            if input.len() > MAX_INPUT_LEN {
                return;
            }
            let text = String::from_utf8_lossy(input);
            if let Ok(expression) = syn::parse_str::<syn::Expr>(&text) {
                let accepted = parse_ident_value(&expression, "identity").is_ok();
                let expected = matches!(
                    expression,
                    syn::Expr::Path(ref path)
                        if path.path.leading_colon.is_none()
                            && path.path.segments.len() == 1
                            && path.path.get_ident().is_some()
                );
                assert_eq!(accepted, expected);
            }
        });
    }
}
