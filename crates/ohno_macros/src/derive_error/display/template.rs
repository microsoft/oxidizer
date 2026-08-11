// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Splitting a `#[display(...)]` template into literal and placeholder segments.
//!
//! A template is parsed before it is judged, so "what the template says" stays apart from "whether
//! that names a field" and neither decision is made halfway through a scan.

/// One piece of a template.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Segment<'a> {
    /// Text copied to the output template verbatim, `{{` and `}}` escapes included.
    Literal(&'a str),
    /// A `{...}` placeholder.
    Placeholder(Placeholder<'a>),
}

/// A `{...}` placeholder.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Placeholder<'a> {
    /// The field named by the placeholder. `None` for `{}`, which consumes the next argument.
    pub(crate) name: Option<&'a str>,
    /// The format spec after the `:`, without the colon.
    pub(crate) spec: Option<&'a str>,
}

impl Placeholder<'_> {
    /// The placeholder as it is written in the lowered template, with the name dropped.
    pub(crate) fn lowered(&self) -> String {
        self.spec.map_or_else(|| "{}".to_owned(), |spec| format!("{{:{spec}}}"))
    }
}

/// Why a template could not be split.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    /// A `{` that no `}` closes.
    UnclosedPlaceholder,
    /// A `}` that no `{` opens.
    StrayClosingBrace,
}

impl Fault {
    /// The diagnostic this fault renders as.
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::UnclosedPlaceholder => {
                "`#[display(...)]` template has a `{` with no matching `}`. \
                 Close the placeholder, or write `{{` for a literal brace"
            }
            Self::StrayClosingBrace => {
                "`#[display(...)]` template has a `}` with no matching `{`. \
                 Open the placeholder, or write `}}` for a literal brace"
            }
        }
    }
}

/// Splits `template` into segments.
///
/// An unbalanced brace is reported rather than repaired. Letting an unterminated `{` run to the end
/// would honor `"{path"` as `"{path}"`, so a typo would render as a working message and never
/// surface. A stray `}` would otherwise be copied into the generated `format!` string, where
/// `rustc` reports it against code the user cannot see.
pub(crate) fn split(template: &str) -> Result<Vec<Segment<'_>>, Fault> {
    let bytes = template.as_bytes();
    let mut segments = Vec::new();
    let mut literal_start = 0;
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'{' if bytes.get(index + 1) == Some(&b'{') => index += 2,
            b'}' if bytes.get(index + 1) == Some(&b'}') => index += 2,
            b'}' => return Err(Fault::StrayClosingBrace),
            b'{' => {
                let close = index
                    + 1
                    + bytes[index + 1..]
                        .iter()
                        .position(|&b| b == b'}')
                        .ok_or(Fault::UnclosedPlaceholder)?;

                if literal_start < index {
                    segments.push(Segment::Literal(&template[literal_start..index]));
                }
                segments.push(Segment::Placeholder(placeholder(&template[index + 1..close])));

                index = close + 1;
                literal_start = index;
            }
            _ => index += 1,
        }
    }

    if literal_start < bytes.len() {
        segments.push(Segment::Literal(&template[literal_start..]));
    }

    Ok(segments)
}

/// Splits a placeholder's contents into its name and its format spec.
fn placeholder(contents: &str) -> Placeholder<'_> {
    let (name, spec) = match contents.find(':') {
        Some(colon) => (&contents[..colon], Some(&contents[colon + 1..])),
        None => (contents, None),
    };

    Placeholder {
        name: (!name.is_empty()).then_some(name),
        spec,
    }
}

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
