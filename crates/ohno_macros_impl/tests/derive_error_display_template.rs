// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

use ohno_macros_impl::derive_error::display::template::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Segment<'_> {
        Segment::Placeholder(Placeholder {
            name: Some(name),
            spec: None,
        })
    }

    #[test]
    fn a_plain_template_is_one_literal() {
        assert_eq!(split("plain text"), Ok(vec![Segment::Literal("plain text")]));
    }

    #[test]
    fn an_empty_template_has_no_segments() {
        assert_eq!(split(""), Ok(Vec::new()));
    }

    #[test]
    fn a_named_placeholder_is_split_out() {
        assert_eq!(
            split("failed for {path}!"),
            Ok(vec![Segment::Literal("failed for "), named("path"), Segment::Literal("!")])
        );
    }

    #[test]
    fn a_positional_placeholder_carries_no_name() {
        assert_eq!(split("{}"), Ok(vec![Segment::Placeholder(Placeholder { name: None, spec: None })]));
    }

    #[test]
    #[expect(clippy::literal_string_with_formatting_args, reason = "the format spec is the subject of the test")]
    fn a_format_spec_is_kept() {
        assert_eq!(
            split("{rules:?} {:>8}"),
            Ok(vec![
                Segment::Placeholder(Placeholder {
                    name: Some("rules"),
                    spec: Some("?")
                }),
                Segment::Literal(" "),
                Segment::Placeholder(Placeholder {
                    name: None,
                    spec: Some(">8")
                }),
            ])
        );
    }

    #[test]
    fn a_raw_identifier_keeps_its_prefix() {
        assert_eq!(split("{r#type}"), Ok(vec![named("r#type")]));
    }

    #[test]
    fn a_tuple_index_is_a_name() {
        assert_eq!(split("{0}"), Ok(vec![named("0")]));
    }

    #[test]
    fn escapes_stay_in_the_literal() {
        assert_eq!(
            split("{{{path}}}"),
            Ok(vec![Segment::Literal("{{"), named("path"), Segment::Literal("}}")])
        );
    }

    #[test]
    fn an_escape_only_template_is_one_literal() {
        assert_eq!(split("{{}}"), Ok(vec![Segment::Literal("{{}}")]));
    }

    #[test]
    fn an_escape_between_text_is_skipped_whole() {
        // Both braces of an escape are consumed together. Skipping any other distance re-reads the
        // second brace as if it opened or closed a placeholder.
        assert_eq!(split("a{{b"), Ok(vec![Segment::Literal("a{{b")]));
        assert_eq!(split("a}}b"), Ok(vec![Segment::Literal("a}}b")]));
    }

    #[test]
    fn scanning_resumes_after_the_closing_brace() {
        assert_eq!(split("{a}b{c}"), Ok(vec![named("a"), Segment::Literal("b"), named("c")]));
    }

    #[test]
    fn an_unclosed_placeholder_is_a_fault() {
        assert_eq!(split("bad path: {path"), Err(Fault::UnclosedPlaceholder));
    }

    #[test]
    fn a_stray_closing_brace_is_a_fault() {
        assert_eq!(split("bad path: path}"), Err(Fault::StrayClosingBrace));
    }

    #[test]
    fn faults_carry_their_message() {
        assert!(Fault::UnclosedPlaceholder.message().contains("with no matching `}`"));
        assert!(Fault::StrayClosingBrace.message().contains("with no matching `{`"));
    }

    #[test]
    #[expect(
        clippy::literal_string_with_formatting_args,
        reason = "the lowered placeholder is the subject of the test"
    )]
    fn a_placeholder_lowers_to_a_positional_one() {
        assert_eq!(
            Placeholder {
                name: Some("path"),
                spec: None
            }
            .lowered(),
            "{}"
        );
        assert_eq!(
            Placeholder {
                name: Some("path"),
                spec: Some("?")
            }
            .lowered(),
            "{:?}"
        );
    }
}
