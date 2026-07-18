// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the public `http_path_template` parsing API.

use http_path_template::{Grammar, ParseError, PathTemplate, Segment};

/// A [`ParseError`] category predicate (e.g. [`ParseError::is_empty_verb`]), used
/// by the table-driven parse-failure tests.
type ParseErrorPredicate = fn(&ParseError) -> bool;

/// Returns the field path and sub-template segments of the [`Segment::Variable`]
/// a template is expected to contain at `index`, or `None` if that segment is
/// not a variable.
fn variable_at<'a>(template: &PathTemplate<'a>, index: usize) -> Option<(Vec<&'a str>, Vec<Segment<'a>>)> {
    match template.segments()[index] {
        Segment::Variable(variable) => Some((variable.field_path().split('.').collect(), variable.segments().collect())),
        _ => None,
    }
}

#[test]
fn parses_simple_literals() {
    let t = PathTemplate::parse("/v1/shelves", Grammar::default()).expect("valid");
    assert_eq!(t.segments(), &[Segment::Literal("v1"), Segment::Literal("shelves")]);
    assert!(t.verb().is_none());
}

#[test]
fn parses_root() {
    let t = PathTemplate::parse("/", Grammar::default()).expect("root is valid");
    assert!(t.segments().is_empty());
    assert!(t.verb().is_none());
}

#[test]
fn parses_single_wildcard_segment() {
    let t = PathTemplate::parse("/v1/*", Grammar::default()).expect("valid");
    assert_eq!(t.segments(), &[Segment::Literal("v1"), Segment::Single]);
}

#[test]
fn parses_variable_shorthand_to_single() {
    let t = PathTemplate::parse("/v1/shelves/{shelf}", Grammar::default()).expect("valid");
    assert_eq!(t.segments()[0], Segment::Literal("v1"));
    assert_eq!(t.segments()[1], Segment::Literal("shelves"));
    let (field_path, segments) = variable_at(&t, 2).expect("segment is a variable");
    assert_eq!(field_path, ["shelf"]);
    assert_eq!(segments, [Segment::Single]);
}

#[test]
fn parses_nested_field_path() {
    let t = PathTemplate::parse("/v1/{shelf.id}", Grammar::default()).expect("valid");
    assert_eq!(t.segments()[0], Segment::Literal("v1"));
    let (field_path, segments) = variable_at(&t, 1).expect("segment is a variable");
    assert_eq!(field_path, ["shelf", "id"]);
    assert_eq!(segments, [Segment::Single]);
}

#[test]
fn parses_variable_subtemplate_and_rest() {
    let t = PathTemplate::parse("/v1/{name=books/**}", Grammar::default()).expect("valid");
    assert_eq!(t.segments()[0], Segment::Literal("v1"));
    let (field_path, segments) = variable_at(&t, 1).expect("segment is a variable");
    assert_eq!(field_path, ["name"]);
    assert_eq!(segments, [Segment::Literal("books"), Segment::Rest]);
}

#[test]
fn parses_trailing_rest() {
    let t = PathTemplate::parse("/v1/files/**", Grammar::default()).expect("valid");
    assert_eq!(t.segments().last(), Some(&Segment::Rest));
}

#[test]
fn variable_accessors_return_field_and_subtemplate() {
    let t = PathTemplate::parse("/v1/{shelf.id=books/*}", Grammar::default()).expect("valid");
    let (field_path, segments) = variable_at(&t, 1).expect("segment is a variable");
    assert_eq!(field_path, ["shelf", "id"]);
    assert_eq!(segments, [Segment::Literal("books"), Segment::Single]);
}

#[test]
fn parses_custom_verb() {
    let t = PathTemplate::parse("/v1/{name=shelves/*}:read", Grammar::default()).expect("valid");
    assert_eq!(t.verb(), Some("read"));
}

#[test]
fn requires_leading_slash() {
    assert!(
        PathTemplate::parse("v1/x", Grammar::default())
            .unwrap_err()
            .is_missing_leading_slash()
    );
}

#[test]
fn rejects_empty_segment() {
    assert!(PathTemplate::parse("/a//b", Grammar::default()).unwrap_err().is_empty_segment());
}

#[test]
fn rejects_rest_not_last() {
    assert!(PathTemplate::parse("/a/**/b", Grammar::default()).unwrap_err().is_rest_not_last());
}

#[test]
fn rejects_rest_not_last_inside_variable() {
    assert!(
        PathTemplate::parse("/a/{x=**}/b", Grammar::default())
            .unwrap_err()
            .is_rest_not_last()
    );
}

#[test]
fn rejects_nested_variable() {
    assert!(
        PathTemplate::parse("/a/{x={y}}", Grammar::default())
            .unwrap_err()
            .is_nested_variable()
    );
}

#[test]
fn rejects_invalid_field_name() {
    assert!(
        PathTemplate::parse("/a/{1bad}", Grammar::default())
            .unwrap_err()
            .is_invalid_field_name()
    );
}

#[test]
fn rejects_empty_verb() {
    assert!(PathTemplate::parse("/a/b:", Grammar::default()).unwrap_err().is_empty_verb());
}

#[test]
fn rejects_multiple_verbs() {
    assert!(PathTemplate::parse("/a:b:c", Grammar::default()).unwrap_err().is_multiple_verbs());
}

#[test]
fn rejects_verb_containing_slash() {
    assert!(PathTemplate::parse("/v1/a:b/c", Grammar::default()).unwrap_err().is_invalid_verb());
}

#[test]
fn rejects_verb_with_non_literal_characters() {
    // A verb is a LITERAL, so wildcard/brace characters are rejected. Braces are
    // especially important: a stray `{` must not unbalance the colon-depth scan.
    assert!(PathTemplate::parse("/a:bad*", Grammar::default()).unwrap_err().is_invalid_verb());
    assert!(PathTemplate::parse("/a:bad%GG", Grammar::default()).unwrap_err().is_invalid_verb());
    assert!(PathTemplate::parse("/a:bad}", Grammar::default()).unwrap_err().is_invalid_verb());
    assert!(
        PathTemplate::parse("/a:bad{:still_bad", Grammar::default())
            .unwrap_err()
            .is_invalid_verb()
    );
}

#[test]
fn rejects_unbalanced_braces() {
    assert!(PathTemplate::parse("/a/{x", Grammar::default()).unwrap_err().is_unbalanced_braces());
}

#[test]
fn parse_error_display_covers_every_kind() {
    let cases: [(&str, ParseErrorPredicate, &str); 11] = [
        (
            "no-leading-slash",
            ParseError::is_missing_leading_slash,
            "template must begin with '/'",
        ),
        ("/a//b", ParseError::is_empty_segment, "template contains an empty path segment"),
        ("/a/{x", ParseError::is_unbalanced_braces, "template contains unbalanced '{' or '}'"),
        (
            "/a/{x={y}}",
            ParseError::is_nested_variable,
            "variable sub-templates may not contain nested variables",
        ),
        ("/a/{=x}", ParseError::is_empty_field_path, "variable has an empty field path"),
        (
            "/a/{1bad}",
            ParseError::is_invalid_field_name,
            "variable field path contains an invalid identifier",
        ),
        (
            "/a*b",
            ParseError::is_invalid_literal,
            "literal contains an invalid character or percent escape",
        ),
        ("/a/**/b", ParseError::is_rest_not_last, "'**' may only appear as the final segment"),
        ("/a/b:", ParseError::is_empty_verb, "custom verb after ':' is empty"),
        (
            "/v1/a:b/c",
            ParseError::is_invalid_verb,
            "custom verb after ':' is not a valid path-template literal",
        ),
        (
            "/a:b:c",
            ParseError::is_multiple_verbs,
            "template contains more than one ':' verb separator",
        ),
    ];
    for (template, predicate, message) in cases {
        let err = PathTemplate::parse(template, Grammar::default()).unwrap_err();
        assert!(predicate(&err), "kind for {template:?}");
        assert_eq!(err.to_string(), message, "message for {template:?}");
    }
}

#[test]
fn each_kind_predicate_matches_only_its_own_kind() {
    // One representative template per error kind, paired with the predicate that
    // must recognise it.
    let cases: [(&str, ParseErrorPredicate); 11] = [
        ("no-leading-slash", ParseError::is_missing_leading_slash),
        ("/a//b", ParseError::is_empty_segment),
        ("/a/{x", ParseError::is_unbalanced_braces),
        ("/a/{x={y}}", ParseError::is_nested_variable),
        ("/a/{=x}", ParseError::is_empty_field_path),
        ("/a/{1bad}", ParseError::is_invalid_field_name),
        ("/a*b", ParseError::is_invalid_literal),
        ("/a/**/b", ParseError::is_rest_not_last),
        ("/a/b:", ParseError::is_empty_verb),
        ("/v1/a:b/c", ParseError::is_invalid_verb),
        ("/a:b:c", ParseError::is_multiple_verbs),
    ];
    let all_predicates: [ParseErrorPredicate; 11] = cases.map(|(_, predicate)| predicate);

    for (template, predicate) in cases {
        let err = PathTemplate::parse(template, Grammar::default()).unwrap_err();
        // The kind's own predicate holds, and no other predicate does — so each
        // predicate is exclusive to its kind (a predicate hard-wired to `true`
        // would make some other kind's error match two predicates).
        assert!(predicate(&err), "own predicate for {template:?}");
        let matches = all_predicates.iter().filter(|check| check(&err)).count();
        assert_eq!(matches, 1, "exactly one predicate matches {template:?}");
    }
}

#[test]
fn colon_inside_braces_is_not_a_verb() {
    // The `:` sits inside the variable's braces, so it must not be read as a
    // trailing custom verb.
    let t = PathTemplate::parse("/a/{b=c:d}", Grammar::default()).expect("valid");
    assert!(t.verb().is_none());
    let (field_path, segments) = variable_at(&t, 1).expect("segment is a variable");
    assert_eq!(field_path, ["b"]);
    assert_eq!(segments, [Segment::Literal("c:d")]);
}

#[test]
fn accepts_underscore_led_field_name() {
    // Covers `_` as both the leading and a subsequent identifier character.
    let t = PathTemplate::parse("/v1/{_my_id}", Grammar::default()).expect("underscore field name is valid");
    let (field_path, _) = variable_at(&t, 1).expect("segment is a variable");
    assert_eq!(field_path, ["_my_id"]);
}

#[test]
fn rejects_embedded_braces_in_literal() {
    assert!(PathTemplate::parse("/x/a{b}", Grammar::default()).unwrap_err().is_invalid_literal());
}

#[test]
fn rejects_trailing_chars_after_variable_close() {
    assert!(
        PathTemplate::parse("/v1/{a}b", Grammar::default())
            .unwrap_err()
            .is_invalid_literal()
    );
}

#[test]
fn rejects_unbalanced_closing_brace() {
    assert!(PathTemplate::parse("/a/}", Grammar::default()).unwrap_err().is_unbalanced_braces());
}

#[test]
fn rejects_empty_subtemplate() {
    assert!(PathTemplate::parse("/a/{x=}", Grammar::default()).unwrap_err().is_empty_segment());
}

#[test]
fn rejects_empty_subtemplate_segment() {
    assert!(
        PathTemplate::parse("/a/{x=a//b}", Grammar::default())
            .unwrap_err()
            .is_empty_segment()
    );
}

#[test]
fn rejects_wildcard_in_subtemplate_literal() {
    assert!(
        PathTemplate::parse("/a/{x=a*b}", Grammar::default())
            .unwrap_err()
            .is_invalid_literal()
    );
}

#[test]
fn validates_uri_literal_characters_and_percent_escapes() {
    for template in ["/a b", "/a?b", "/a#b", "/café", "/bad%", "/bad%2", "/bad%2x", "/{x=bad%GG}"] {
        let error = PathTemplate::parse(template, Grammar::default()).expect_err(template);
        assert!(error.is_invalid_literal(), "wrong error for {template:?}: {error}");
    }

    for template in [
        "/-._~!$&'()+,;=@",
        "/caf%C3%A9",
        "/lower%2fupper%2F",
        "/{x=part%2Fname}",
        "/{x=part:detail}",
        "/a:verb%2Fname",
    ] {
        PathTemplate::parse(template, Grammar::default()).unwrap_or_else(|error| {
            panic!("valid URI literal {template:?}: {error}");
        });
    }

    for template in ["/bad%GG{x}", "/{x}bad%GG", "/pré{x}"] {
        let error = PathTemplate::parse(template, Grammar::default().with_segment_affixes()).expect_err(template);
        assert!(error.is_invalid_literal(), "wrong affix error for {template:?}: {error}");
    }

    PathTemplate::parse("/pre%2F{x}suf%2f", Grammar::default().with_segment_affixes())
        .expect("percent escapes are valid in affix literals");
}

#[test]
fn variable_segment_iterator_has_an_exact_fused_length() {
    let template = PathTemplate::parse("/{x=one/*/**}", Grammar::default()).expect("valid");
    let Segment::Variable(variable) = template.segments()[0] else {
        panic!("expected variable");
    };
    let mut segments = variable.segments();
    assert_eq!(segments.len(), 3);
    assert_eq!(segments.size_hint(), (3, Some(3)));
    assert_eq!(segments.next(), Some(Segment::Literal("one")));
    assert_eq!(segments.len(), 2);
    assert_eq!(segments.next(), Some(Segment::Single));
    assert_eq!(segments.next(), Some(Segment::Rest));
    assert_eq!(segments.len(), 0);
    assert_eq!(segments.next(), None);
    assert_eq!(segments.next(), None);
}

#[test]
fn display_renders_canonical_template() {
    let cases = [
        // Root template with no segments and no verb.
        ("/", "/"),
        // Root template carrying only a custom verb.
        ("/:ping", "/:ping"),
        // Literals, `*`, and a trailing `**`.
        ("/v1/shelves/*/**", "/v1/shelves/*/**"),
        // Variable shorthand stays `{field}`, dotted field paths are preserved.
        ("/v1/shelves/{shelf.id}", "/v1/shelves/{shelf.id}"),
        // Explicit `{field=*}` collapses to the `{field}` shorthand.
        ("/v1/{name=*}", "/v1/{name}"),
        // Variable sub-templates with literals, `*`, and `**`.
        ("/v1/{name=books/*}", "/v1/{name=books/*}"),
        ("/v1/{name=books/**}", "/v1/{name=books/**}"),
        // A `:` inside braces is part of a path-segment literal, not a verb.
        ("/a/{b=c:d}", "/a/{b=c:d}"),
        // Full template combining variables, a `**` sub-template, and a verb.
        (
            "/shelves/{shelf}/books/{book=**}:archive",
            "/shelves/{shelf}/books/{book=**}:archive",
        ),
    ];
    for (template, expected) in cases {
        let rendered = PathTemplate::parse(template, Grammar::default()).expect("valid").to_string();
        assert_eq!(rendered, expected, "display for {template:?}");
    }
}

#[test]
fn display_round_trips_through_parse() {
    let templates = [
        "/",
        "/:ping",
        "/v1/shelves/*/**",
        "/v1/shelves/{shelf.id}",
        "/v1/{name=*}",
        "/v1/{name=books/*}",
        "/v1/{name=books/**}",
        "/a/{b=c:d}",
        "/shelves/{shelf}/books/{book=**}:archive",
    ];
    for template in templates {
        let parsed = PathTemplate::parse(template, Grammar::default()).expect("valid");
        let rendered = parsed.to_string();
        let reparsed = PathTemplate::parse(&rendered, Grammar::default()).expect("rendered template is valid");
        assert_eq!(parsed, reparsed, "round trip for {template:?}");
    }
}

#[test]
fn extended_parses_suffix_prefix_and_both() {
    // Suffix only.
    assert_eq!(
        PathTemplate::parse("/files/{name}.json", Grammar::default().with_segment_affixes())
            .expect("valid")
            .segments()[1],
        Segment::Affix {
            prefix: "",
            name: "name",
            suffix: ".json",
        }
    );
    // Prefix only.
    assert_eq!(
        PathTemplate::parse("/v{version}/x", Grammar::default().with_segment_affixes())
            .expect("valid")
            .segments()[0],
        Segment::Affix {
            prefix: "v",
            name: "version",
            suffix: "",
        }
    );
    // Prefix and suffix, dotted field name.
    assert_eq!(
        PathTemplate::parse("/img-{img.id}.png", Grammar::default().with_segment_affixes())
            .expect("valid")
            .segments()[0],
        Segment::Affix {
            prefix: "img-",
            name: "img.id",
            suffix: ".png",
        }
    );
}

#[test]
fn strict_rejects_extended_syntax() {
    PathTemplate::parse("/files/{name}.json", Grammar::default()).expect_err("rejected");
    PathTemplate::parse("/v{version}/x", Grammar::default()).expect_err("rejected");
    PathTemplate::parse("/img-{id}.png", Grammar::default()).expect_err("rejected");
}

#[test]
fn extended_still_accepts_strict_templates() {
    let strict = "/v1/shelves/{shelf}/books/{book=**}:archive";
    assert_eq!(
        PathTemplate::parse(strict, Grammar::default()).expect("valid"),
        PathTemplate::parse(strict, Grammar::default().with_segment_affixes()).expect("valid"),
    );
}

#[test]
fn extended_rejects_two_params_in_one_segment() {
    PathTemplate::parse("/a{x}b{y}c", Grammar::default().with_segment_affixes()).expect_err("rejected");
}

#[test]
fn extended_rejects_wildcard_in_affix() {
    PathTemplate::parse("/a*{x}", Grammar::default().with_segment_affixes()).expect_err("rejected");
    PathTemplate::parse("/{x}*b", Grammar::default().with_segment_affixes()).expect_err("rejected");
}

#[test]
fn extended_rejects_empty_or_invalid_affix_field() {
    PathTemplate::parse("/a{}b", Grammar::default().with_segment_affixes()).expect_err("rejected");
    PathTemplate::parse("/a{1bad}b", Grammar::default().with_segment_affixes()).expect_err("rejected");
}

#[test]
fn extended_affix_round_trips_through_display() {
    for template in ["/files/{name}.json", "/v{version}/x", "/img-{img.id}.png"] {
        let parsed = PathTemplate::parse(template, Grammar::default().with_segment_affixes()).expect("valid");
        assert_eq!(parsed.to_string(), template, "display for {template:?}");
        let rendered = parsed.to_string();
        let reparsed = PathTemplate::parse(&rendered, Grammar::default().with_segment_affixes()).expect("valid");
        assert_eq!(parsed, reparsed, "round trip for {template:?}");
    }
}

#[test]
fn extended_rejects_closing_brace_before_opening() {
    // A `}` appearing before the `{` (one of each) is malformed, not an affix.
    PathTemplate::parse("/a}b{c", Grammar::default().with_segment_affixes()).expect_err("rejected");
}
