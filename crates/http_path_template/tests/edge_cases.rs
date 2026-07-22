// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exhaustive edge-case and correctness tests for `http_path_template`.
//!
//! These tests exercise the `google.api.http` path-template grammar
//! ```text
//! Template  = "/" Segments [ Verb ] ;
//! Segments  = Segment { "/" Segment } ;
//! Segment   = "*" | "**" | LITERAL | Variable ;
//! Variable  = "{" FieldPath [ "=" Segments ] "}" ;
//! FieldPath = IDENT { "." IDENT } ;
//! Verb      = ":" LITERAL ;
//! ```
//! against boundary conditions, the `**`-must-be-last rule, brace balancing,
//! field-path identifier rules, the borrowed/zero-copy invariants, and
//! `Display` round-trip totality.

// Path templates contain `{field}` braces that superficially resemble format
// placeholders; the literals here are intentional grammar inputs, not format
// strings.
#![expect(
    clippy::literal_string_with_formatting_args,
    reason = "template literals contain `{field}` braces that are grammar input, not format args"
)]

use http_path_template::{Grammar, ParseError, PathTemplate, Segment};

fn strict() -> Grammar {
    Grammar::default()
}

fn ext() -> Grammar {
    Grammar::default().with_segment_affixes()
}

fn parse(s: &str) -> Result<PathTemplate<'_>, ParseError> {
    PathTemplate::parse(s, strict())
}

// ---------------------------------------------------------------------------
// Valid templates: structural spot-checks.
// ---------------------------------------------------------------------------

#[test]
fn root_has_no_segments_and_no_verb() {
    let t = parse("/").expect("root");
    assert!(t.segments().is_empty());
    assert_eq!(t.verb(), None);
}

#[test]
fn root_with_only_a_verb() {
    let t = parse("/:ping").expect("root verb");
    assert!(t.segments().is_empty());
    assert_eq!(t.verb(), Some("ping"));
}

#[test]
fn single_star_and_double_star() {
    assert_eq!(parse("/*").expect("star").segments(), &[Segment::Single]);
    assert_eq!(parse("/**").expect("rest").segments(), &[Segment::Rest]);
}

#[test]
fn double_star_may_be_last_after_literals() {
    let t = parse("/a/b/**").expect("valid");
    assert_eq!(t.segments(), &[Segment::Literal("a"), Segment::Literal("b"), Segment::Rest]);
}

#[test]
fn double_star_last_with_a_verb_is_allowed() {
    let t = parse("/a/**:watch").expect("valid");
    assert_eq!(t.segments(), &[Segment::Literal("a"), Segment::Rest]);
    assert_eq!(t.verb(), Some("watch"));
}

#[test]
fn shorthand_equals_explicit_star() {
    // `{v}` is exactly `{v=*}`; they must produce equal ASTs.
    assert_eq!(parse("/{v}").expect("a"), parse("/{v=*}").expect("b"));
}

#[test]
fn variable_field_path_is_the_raw_dotted_string() {
    let t = parse("/{a.b.c}").expect("valid");
    let Segment::Variable(v) = t.segments()[0] else {
        panic!("variable")
    };
    assert_eq!(v.field_path(), "a.b.c");
    assert!(v.segments().eq([Segment::Single]));
}

#[test]
fn variable_sub_template_segments() {
    let t = parse("/{name=shelves/*/books/**}").expect("valid");
    let Segment::Variable(v) = t.segments()[0] else {
        panic!("variable")
    };
    assert_eq!(v.field_path(), "name");
    assert!(v.segments().eq([
        Segment::Literal("shelves"),
        Segment::Single,
        Segment::Literal("books"),
        Segment::Rest,
    ]));
}

#[test]
fn colon_inside_braces_is_a_literal_not_a_verb() {
    let t = parse("/a/{b=c:d}").expect("valid");
    assert_eq!(t.verb(), None);
    let Segment::Variable(v) = t.segments()[1] else {
        panic!("variable")
    };
    assert!(v.segments().eq([Segment::Literal("c:d")]));
}

#[test]
fn top_level_colon_is_the_verb_separator() {
    let t = parse("/a:b").expect("valid");
    assert_eq!(t.segments(), &[Segment::Literal("a")]);
    assert_eq!(t.verb(), Some("b"));
}

#[test]
fn verb_may_contain_underscores_and_digits() {
    assert_eq!(parse("/res:get_v2").expect("valid").verb(), Some("get_v2"));
}

#[test]
fn literals_may_contain_non_ascii() {
    let t = parse("/café/{shelf}").expect("valid");
    assert_eq!(t.segments()[0], Segment::Literal("café"));
}

#[test]
fn returned_slices_borrow_from_the_input() {
    // The literal must be a sub-slice of the *input* buffer, not a copy.
    let input = String::from("/alpha/{beta}");
    let t = PathTemplate::parse(&input, strict()).expect("valid");
    let Segment::Literal(lit) = t.segments()[0] else {
        panic!("literal")
    };
    let input_start = input.as_ptr() as usize;
    let lit_start = lit.as_ptr() as usize;
    assert!(
        lit_start >= input_start && lit_start < input_start + input.len(),
        "literal must point into the input buffer"
    );
    assert_eq!(lit, "alpha");
}

// ---------------------------------------------------------------------------
// Invalid templates: exact error kinds.
// ---------------------------------------------------------------------------

#[test]
fn invalid_templates_report_the_expected_error() {
    type Pred = fn(&ParseError) -> bool;
    let cases: &[(&str, Pred, &str)] = &[
        ("", ParseError::is_missing_leading_slash, "empty string"),
        ("v1/x", ParseError::is_missing_leading_slash, "no leading slash"),
        // Missing leading slash is reported before any later structural error
        // (e.g. an unbalanced brace), since it is the most fundamental problem.
        ("a}", ParseError::is_missing_leading_slash, "no slash, stray brace"),
        ("}", ParseError::is_missing_leading_slash, "bare close brace, no slash"),
        ("a:b", ParseError::is_missing_leading_slash, "no slash, has verb"),
        ("//", ParseError::is_empty_segment, "empty first segment"),
        ("/a//b", ParseError::is_empty_segment, "empty middle segment"),
        ("/a/", ParseError::is_empty_segment, "empty trailing segment"),
        ("/**/a", ParseError::is_rest_not_last, "** before literal"),
        ("/**/**", ParseError::is_rest_not_last, "** before **"),
        ("/{a=**}/b", ParseError::is_rest_not_last, "** in var, then literal"),
        ("/{a=b/**/c}", ParseError::is_rest_not_last, "** mid sub-template"),
        ("/{a=**/c}", ParseError::is_rest_not_last, "** first of two sub-segments"),
        ("/{}", ParseError::is_empty_field_path, "empty braces"),
        ("/{=x}", ParseError::is_empty_field_path, "empty field before ="),
        ("/{a=}", ParseError::is_empty_segment, "empty sub-template"),
        ("/{a=b//c}", ParseError::is_empty_segment, "empty sub-segment"),
        ("/{a.}", ParseError::is_invalid_field_name, "trailing dot"),
        ("/{.a}", ParseError::is_invalid_field_name, "leading dot"),
        ("/{a..b}", ParseError::is_invalid_field_name, "double dot"),
        ("/{1a}", ParseError::is_invalid_field_name, "digit-leading ident"),
        ("/{a-b}", ParseError::is_invalid_field_name, "hyphen in ident"),
        ("/{a b}", ParseError::is_invalid_field_name, "space in ident"),
        ("/{a={b}}", ParseError::is_nested_variable, "nested variable"),
        ("/{a", ParseError::is_unbalanced_braces, "unclosed brace"),
        ("/a}", ParseError::is_unbalanced_braces, "stray close brace"),
        ("/}a", ParseError::is_unbalanced_braces, "leading close brace"),
        // A stray `}` before a `:` verb separator is the first structural error
        // and wins over the (otherwise empty) verb.
        ("/a}:", ParseError::is_unbalanced_braces, "stray brace before empty verb"),
        ("/a}:b", ParseError::is_unbalanced_braces, "stray brace before verb"),
        ("/a{b}c", ParseError::is_unbalanced_braces, "embedded braces (strict)"),
        ("/a*b", ParseError::is_invalid_literal, "star in literal"),
        ("/*a", ParseError::is_invalid_literal, "star-prefixed literal"),
        ("/{a=*b}", ParseError::is_invalid_literal, "star in sub-literal"),
        ("/a:", ParseError::is_empty_verb, "empty verb"),
        ("/a:b:c", ParseError::is_multiple_verbs, "two verbs"),
        ("/a:b/c", ParseError::is_invalid_verb, "verb contains slash"),
        ("/a:*", ParseError::is_invalid_verb, "verb is a star"),
        ("/a:{b}", ParseError::is_invalid_verb, "verb contains brace"),
        ("/a:b}", ParseError::is_invalid_verb, "trailing brace in verb"),
    ];

    for (template, pred, why) in cases {
        let err = parse(template).expect_err(why);
        assert!(pred(&err), "wrong error kind for {template:?} ({why}): {err}");
    }
}

// ---------------------------------------------------------------------------
// Extended grammar (intra-segment affixes).
// ---------------------------------------------------------------------------

#[test]
fn extended_affix_variants() {
    // suffix only, prefix only, both, and empty prefix.
    let cases = [
        ("/files/{name}.json", "", "name", ".json"),
        ("/v{version}/x", "v", "version", ""),
        ("/img-{id}.png", "img-", "id", ".png"),
        ("/{id}.png", "", "id", ".png"),
        ("/x-{a.b}-y", "x-", "a.b", "-y"),
    ];
    for (template, prefix, name, suffix) in cases {
        let t = PathTemplate::parse(template, ext()).expect(template);
        let seg = t.segments().iter().find(|s| matches!(s, Segment::Affix { .. })).expect("affix");
        assert_eq!(*seg, Segment::Affix { prefix, name, suffix }, "for {template}");
    }
}

#[test]
fn extended_affix_rejections() {
    type Pred = fn(&ParseError) -> bool;
    let cases: &[(&str, Pred)] = &[
        ("/a{x}b{y}c", ParseError::is_unbalanced_braces),
        ("/a{}b", ParseError::is_empty_field_path),
        ("/a{1bad}b", ParseError::is_invalid_field_name),
        ("/a*{x}", ParseError::is_invalid_literal),
        ("/{x}*b", ParseError::is_invalid_literal),
        ("/a}b{c", ParseError::is_unbalanced_braces),
    ];
    for (template, pred) in cases {
        let err = PathTemplate::parse(template, ext()).expect_err(template);
        assert!(pred(&err), "wrong error kind for {template:?}: {err}");
    }
}

#[test]
fn grammar_new_is_the_strict_default() {
    // `Grammar::new()` is the strict `google.api.http` grammar, identical to the
    // `Default`. (Covered here as a regular test since doctests are excluded from
    // coverage.)
    let strict = Grammar::new();
    assert_eq!(strict, Grammar::default());
    assert!(!strict.segment_affixes());

    // Enabling affixes flips the accessor and leaves the (Copy) original strict.
    let extended = strict.with_segment_affixes();
    assert!(extended.segment_affixes());
    assert!(!strict.segment_affixes());
    assert_ne!(strict, extended);
}

#[test]
fn extended_grammar_still_accepts_everything_strict_does() {
    for template in ["/", "/*", "/**", "/v1/{shelf}", "/{name=books/**}", "/a/{b=c:d}:verb"] {
        let strict_result = PathTemplate::parse(template, strict()).expect(template);
        let extended_result = PathTemplate::parse(template, ext()).expect(template);
        assert_eq!(strict_result, extended_result, "extended grammar must be a superset for {template}");
    }
}

#[test]
fn strict_grammar_rejects_affixes() {
    for template in ["/files/{name}.json", "/v{version}/x", "/img-{id}.png"] {
        assert!(parse(template).is_err(), "strict must reject affix {template}");
        assert!(PathTemplate::parse(template, ext()).is_ok(), "extended must accept {template}");
    }
}

// ---------------------------------------------------------------------------
// Display round-trip totality.
// ---------------------------------------------------------------------------

const VALID_STRICT: &[&str] = &[
    "/",
    "/:ping",
    "/v1",
    "/v1/shelves",
    "/*",
    "/**",
    "/a/*/b/**",
    "/{shelf}",
    "/{shelf.id}",
    "/{name=*}",
    "/{name=**}",
    "/{name=books/*}",
    "/{name=books/**}",
    "/{name=a/b/c/**}",
    "/v1/shelves/{shelf}/books/{book=**}",
    "/v1/shelves/{shelf}/books/{book}:archive",
    "/a/{b=c:d}",
    "/café/{shelf}",
    "/a:b",
    "/x/{y}/**:read",
];

#[test]
fn display_round_trips_and_is_idempotent() {
    for template in VALID_STRICT {
        let parsed = parse(template).unwrap_or_else(|e| panic!("valid {template:?}: {e}"));
        let rendered = parsed.to_string();
        let reparsed = parse(&rendered).unwrap_or_else(|e| panic!("reparse {rendered:?}: {e}"));
        // The AST is stable under render→parse.
        assert_eq!(parsed, reparsed, "AST round trip for {template:?}");
        // Rendering is idempotent (a fixed point after the first render).
        assert_eq!(rendered, reparsed.to_string(), "idempotent display for {template:?}");
    }
}

#[test]
fn display_collapses_explicit_single_star_to_shorthand() {
    assert_eq!(parse("/{name=*}").expect("valid").to_string(), "/{name}");
    // A dotted field path is preserved verbatim.
    assert_eq!(parse("/{a.b.c}").expect("valid").to_string(), "/{a.b.c}");
}

#[test]
fn extended_affix_round_trips() {
    for template in ["/files/{name}.json", "/v{version}/x", "/img-{a.b}.png", "/{id}.tar.gz"] {
        let parsed = PathTemplate::parse(template, ext()).expect(template);
        assert_eq!(parsed.to_string(), template, "display for {template}");
    }
}

// ---------------------------------------------------------------------------
// Boundary / stress.
// ---------------------------------------------------------------------------

#[test]
fn many_segments_parse_and_round_trip() {
    let template: String = core::iter::once(String::new())
        .chain((0..200).map(|i| format!("seg{i}")))
        .collect::<Vec<_>>()
        .join("/");
    let parsed = PathTemplate::parse(&template, strict()).expect("many segments");
    assert_eq!(parsed.segments().len(), 200);
    assert_eq!(parsed.to_string(), template);
}

#[test]
fn deeply_dotted_field_path() {
    let field: String = (0..50).map(|i| format!("f{i}")).collect::<Vec<_>>().join(".");
    let template = format!("/{{{field}}}");
    let parsed = PathTemplate::parse(&template, strict()).expect("deep field path");
    let Segment::Variable(v) = parsed.segments()[0] else {
        panic!("variable")
    };
    assert_eq!(v.field_path(), field);
}

// ---------------------------------------------------------------------------
// Exhaustive fuzzing over short strings of structural characters.
//
// For every string up to a bounded length over an alphabet of grammar-
// significant characters, parsing must (1) never panic (proving all slicing is
// UTF-8-safe and free of off-by-one bounds bugs) and (2) whenever it succeeds,
// `Display` must round-trip to an equal AST and be idempotent. This exercises
// the parser across its entire short-input space under both grammars.
// ---------------------------------------------------------------------------

/// Applies `f` to every string of length `0..=max_len` over `alphabet`.
fn for_each_string(alphabet: &[char], max_len: usize, f: &mut impl FnMut(&str)) {
    fn rec(alphabet: &[char], remaining: usize, buf: &mut String, f: &mut impl FnMut(&str)) {
        f(buf);
        if remaining == 0 {
            return;
        }
        for &c in alphabet {
            buf.push(c);
            rec(alphabet, remaining - 1, buf, f);
            buf.pop();
        }
    }
    let mut buf = String::new();
    rec(alphabet, max_len, &mut buf, f);
}

#[expect(clippy::panic, reason = "test helper: a failed re-parse is a genuine test failure")]
fn check_no_panic_and_round_trip(template: &str, grammar: Grammar) {
    let Ok(parsed) = PathTemplate::parse(template, grammar) else {
        return;
    };
    // Whatever parsed must render and re-parse to an equal, stable AST.
    let rendered = parsed.to_string();
    let reparsed = PathTemplate::parse(&rendered, grammar)
        .unwrap_or_else(|e| panic!("re-parse of rendered {rendered:?} (from {template:?}) failed: {e}"));
    assert_eq!(parsed, reparsed, "AST round trip for {template:?} -> {rendered:?}");
    assert_eq!(rendered, reparsed.to_string(), "idempotent display for {template:?}");
}

#[test]
#[cfg_attr(miri, ignore = "exhaustive sweep is too slow under Miri; no unsafe to check")]
fn exhaustive_ascii_structural_strings_do_not_panic_and_round_trip() {
    // Grammar-significant ASCII characters plus a plain identifier byte.
    const ALPHABET: &[char] = &['/', '{', '}', '*', ':', '=', '.', 'a'];
    const MAX_LEN: u32 = 6;
    let mut count = 0_u64;
    for_each_string(ALPHABET, MAX_LEN as usize, &mut |s| {
        check_no_panic_and_round_trip(s, strict());
        check_no_panic_and_round_trip(s, ext());
        count += 1;
    });
    // Sanity: we actually covered the whole space (sum of 8^k for k in 0..=6).
    assert_eq!(count, (0..=MAX_LEN).map(|k| 8_u64.pow(k)).sum());
}

#[test]
#[cfg_attr(miri, ignore = "exhaustive sweep is too slow under Miri; no unsafe to check")]
fn exhaustive_multibyte_strings_do_not_panic_on_boundaries() {
    // Include a 2-byte UTF-8 char to stress char-boundary slicing around the
    // ASCII structural delimiters.
    const ALPHABET: &[char] = &['/', '{', '}', '*', '=', 'é'];
    const MAX_LEN: usize = 5;
    for_each_string(ALPHABET, MAX_LEN, &mut |s| {
        check_no_panic_and_round_trip(s, strict());
        check_no_panic_and_round_trip(s, ext());
    });
}
