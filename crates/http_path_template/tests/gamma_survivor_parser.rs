// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Template grammar and canonical rendering regression oracles.

use http_path_template::{Grammar, PathTemplate, Segment};

#[test]
fn gamma_survivor_render_every_segment_and_normalize_shorthand() {
    for (source, rendered) in [
        ("/", "/"),
        ("/:read", "/:read"),
        ("/plain/*/**:read", "/plain/*/**:read"),
        ("/{a=*}/{b=books/*}/{c=**}:archive", "/{a}/{b=books/*}/{c=**}:archive"),
        ("/pre-{a.b}.json/{_id=a%2Fb/*}", "/pre-{a.b}.json/{_id=a%2Fb/*}"),
    ] {
        let parsed = PathTemplate::parse(source, Grammar::default().with_segment_affixes()).unwrap();
        assert_eq!(parsed.to_string(), rendered);
        assert_eq!(
            PathTemplate::parse(rendered, Grammar::default().with_segment_affixes()).unwrap(),
            parsed
        );
    }
    let parsed = PathTemplate::parse("/{book.id=shelves/*/books/**}:get", Grammar::default()).unwrap();
    assert_eq!(parsed.verb(), Some("get"));
    let Segment::Variable(var) = &parsed.segments()[0] else {
        panic!("expected variable")
    };
    assert_eq!(var.field_path(), "book.id");
    assert_eq!(
        var.segments().collect::<Vec<_>>(),
        [
            Segment::Literal("shelves"),
            Segment::Single,
            Segment::Literal("books"),
            Segment::Rest
        ]
    );
}

#[test]
fn gamma_survivor_structural_errors_and_precedence() {
    for (source, expected) in [
        ("a/{", "template must begin with '/'"),
        ("/a//b", "template contains an empty path segment"),
        ("/a/", "template contains an empty path segment"),
        ("/{a", "template contains unbalanced '{' or '}'"),
        ("/a}:*", "template contains unbalanced '{' or '}'"),
        ("/{a={b}}", "variable sub-templates may not contain nested variables"),
        ("/{}", "variable has an empty field path"),
        ("/{=*}", "variable has an empty field path"),
        ("/{a=}", "template contains an empty path segment"),
        ("/{a=x//y}", "template contains an empty path segment"),
        ("/**/*", "'**' may only appear as the final segment"),
        ("/**/**", "'**' may only appear as the final segment"),
        ("/{a=**}/literal", "'**' may only appear as the final segment"),
        ("/{a=**}/{b}", "'**' may only appear as the final segment"),
        ("/{a=**/*}", "'**' may only appear as the final segment"),
        ("/a:", "custom verb after ':' is empty"),
        ("/a:*", "custom verb after ':' is not a valid path-template literal"),
        ("/a:r:s", "template contains more than one ':' verb separator"),
        ("/x-{a}{b}", "template contains unbalanced '{' or '}'"),
        ("/x-{a={b}}", "template contains unbalanced '{' or '}'"),
    ] {
        let error = PathTemplate::parse(source, Grammar::default().with_segment_affixes()).unwrap_err();
        assert_eq!(error.to_string(), expected, "{source}");
    }
    for field in ["1", "a.", ".a", "a..b", "a-b", "é", "a!"] {
        for source in [format!("/{{{field}}}"), format!("/x-{{{field}}}.json")] {
            assert!(
                PathTemplate::parse(&source, Grammar::default().with_segment_affixes())
                    .unwrap_err()
                    .is_invalid_field_name(),
                "{source}"
            );
        }
    }
    assert!(
        PathTemplate::parse("/x-{}", Grammar::default().with_segment_affixes())
            .unwrap_err()
            .is_empty_field_path()
    );
    assert!(
        PathTemplate::parse("/x-{a=*}", Grammar::default().with_segment_affixes())
            .unwrap_err()
            .is_invalid_field_name()
    );
    assert!(PathTemplate::parse("/x-{a}", Grammar::default()).unwrap_err().is_invalid_literal());
    assert!(
        PathTemplate::parse("/**/x-{a}", Grammar::default().with_segment_affixes())
            .unwrap_err()
            .is_rest_not_last()
    );
}

#[test]
fn gamma_survivor_uri_literal_and_percent_boundaries() {
    for literal in ["AZaz09-._~!$&'()+,;=@", "%00%09%aF%Ff", "a%41z"] {
        for source in [format!("/{literal}"), format!("/{{name={literal}}}"), format!("/x:{literal}")] {
            let parsed = PathTemplate::parse(&source, Grammar::default()).unwrap();
            assert_eq!(parsed.to_string(), source);
        }
    }
    for literal in ["%", "%0", "%G0", "%0G", "%FF?", "a*b", "a b", "é", "#", "?"] {
        assert!(
            PathTemplate::parse(&format!("/{literal}"), Grammar::default())
                .unwrap_err()
                .is_invalid_literal(),
            "{literal}"
        );
        assert!(
            PathTemplate::parse(&format!("/x:{literal}"), Grammar::default())
                .unwrap_err()
                .is_invalid_verb(),
            "{literal}"
        );
    }
    for source in ["/bad%G0-{id}", "/{id}-bad%0G"] {
        assert!(
            PathTemplate::parse(source, Grammar::default().with_segment_affixes())
                .unwrap_err()
                .is_invalid_literal()
        );
    }
}
#[test]
fn gamma_survivor_rendering_propagates_each_writer_failure() {
    use std::fmt;
    struct Writer {
        remaining: Option<usize>,
        writes: usize,
    }
    impl fmt::Write for Writer {
        fn write_str(&mut self, _: &str) -> fmt::Result {
            self.writes += 1;
            if let Some(remaining) = &mut self.remaining {
                if *remaining == 0 {
                    return Err(fmt::Error);
                }
                *remaining -= 1;
            }
            Ok(())
        }
    }
    for source in ["/", "/x/y:verb", "/pre-{id}.json", "/{book.id=books/*}", "/{path=**}"] {
        let template = PathTemplate::parse(source, Grammar::default().with_segment_affixes()).unwrap();
        let mut count = Writer {
            remaining: None,
            writes: 0,
        };
        fmt::write(&mut count, format_args!("{template}")).unwrap();
        for remaining in 0..count.writes {
            let mut writer = Writer {
                remaining: Some(remaining),
                writes: 0,
            };
            assert!(fmt::write(&mut writer, format_args!("{template}")).is_err(), "{source} {remaining}");
        }
    }
}
