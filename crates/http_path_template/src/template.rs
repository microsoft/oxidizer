// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`PathTemplate`] type and the template parser.

use crate::error::{ParseError, ParseErrorKind};
use crate::segment::Segment;
use crate::variable::Variable;

/// A parsed `google.api.http` path template.
///
/// Construct one with [`PathTemplate::parse`] and inspect its structure via
/// [`PathTemplate::segments`] / [`PathTemplate::verb`].
///
/// # Examples
///
/// ```
/// use http_path_template::PathTemplate;
///
/// # fn main() -> Result<(), http_path_template::ParseError> {
/// let template = PathTemplate::parse("/v1/shelves/{shelf}:get")?;
/// assert_eq!(template.verb(), Some("get"));
/// assert_eq!(template.segments().len(), 3);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PathTemplate {
    segments: Vec<Segment>,
    verb: Option<String>,
}

impl PathTemplate {
    /// Parses a `google.api.http` path template string.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::PathTemplate;
    ///
    /// # fn main() -> Result<(), http_path_template::ParseError> {
    /// let template = PathTemplate::parse("/shelves/{shelf}/books/{book=**}:archive")?;
    /// assert_eq!(template.verb(), Some("archive"));
    /// assert_eq!(template.segments().len(), 4);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] describing the first structural problem found
    /// (missing leading slash, unbalanced braces, misplaced `**`, …).
    pub fn parse(template: impl AsRef<str>) -> Result<Self, ParseError> {
        let (body, verb) = split_verb(template.as_ref())?;

        let rest = body
            .strip_prefix('/')
            .ok_or_else(|| ParseError::new(ParseErrorKind::MissingLeadingSlash))?;

        let segments = if rest.is_empty() {
            Vec::new()
        } else {
            let raw = split_segments(rest)?;
            let mut parsed = Vec::with_capacity(raw.len());
            for seg in raw {
                parsed.push(parse_segment(seg)?);
            }
            parsed
        };

        // `**` may only be the final atom of the flattened template.
        validate_rest_position(&segments)?;

        Ok(Self { segments, verb })
    }

    /// The top-level segments of the template.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::{PathTemplate, Segment};
    ///
    /// # fn main() -> Result<(), http_path_template::ParseError> {
    /// let template = PathTemplate::parse("/v1/shelves/{shelf}")?;
    /// assert_eq!(template.segments()[0], Segment::Literal(String::from("v1")));
    /// assert!(matches!(template.segments()[2], Segment::Variable(_)));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The custom verb (the `LITERAL` after a trailing `:`), if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::PathTemplate;
    ///
    /// # fn main() -> Result<(), http_path_template::ParseError> {
    /// let template = PathTemplate::parse("/v1/{name=shelves/*}:read")?;
    /// assert_eq!(template.verb(), Some("read"));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn verb(&self) -> Option<&str> {
        self.verb.as_deref()
    }
}

/// Validates that a `**` ([`Segment::Rest`]) appears only as the final atom of
/// the flattened template (including atoms contributed by variable
/// sub-templates), the only position a segment matcher can honor.
fn validate_rest_position(segments: &[Segment]) -> Result<(), ParseError> {
    // Collect, in flattened order, whether each atom is a `**`.
    let mut is_rest = Vec::new();
    for seg in segments {
        match seg {
            Segment::Literal(_) | Segment::Single => is_rest.push(false),
            Segment::Rest => is_rest.push(true),
            Segment::Variable(var) => {
                for sub in var.segments() {
                    is_rest.push(matches!(sub, Segment::Rest));
                }
            }
        }
    }

    for (idx, rest) in is_rest.iter().enumerate() {
        if *rest && idx != is_rest.len() - 1 {
            return Err(ParseError::new(ParseErrorKind::RestNotLast));
        }
    }
    Ok(())
}

/// Splits a template into its body and optional verb at the first top-level `:`.
fn split_verb(template: &str) -> Result<(&str, Option<String>), ParseError> {
    let mut depth = 0_usize;
    let mut verb_at = None;
    for (idx, ch) in template.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => {
                if verb_at.is_some() {
                    return Err(ParseError::new(ParseErrorKind::MultipleVerbs));
                }
                verb_at = Some(idx);
            }
            _ => {}
        }
    }

    match verb_at {
        Some(idx) => {
            let verb = &template[idx + 1..];
            if verb.is_empty() {
                return Err(ParseError::new(ParseErrorKind::EmptyVerb));
            }
            Ok((&template[..idx], Some(verb.to_owned())))
        }
        None => Ok((template, None)),
    }
}

/// Splits a template body (after the leading `/`) into segment strings,
/// treating `/` inside `{...}` as literal.
fn split_segments(body: &str) -> Result<Vec<&str>, ParseError> {
    let mut out = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0_usize;
    for (idx, ch) in body.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                if depth == 0 {
                    return Err(ParseError::new(ParseErrorKind::UnbalancedBraces));
                }
                depth -= 1;
            }
            '/' if depth == 0 => {
                out.push(&body[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(ParseError::new(ParseErrorKind::UnbalancedBraces));
    }
    out.push(&body[start..]);

    for seg in &out {
        if seg.is_empty() {
            return Err(ParseError::new(ParseErrorKind::EmptySegment));
        }
    }
    Ok(out)
}

fn parse_segment(seg: &str) -> Result<Segment, ParseError> {
    if seg == "*" {
        return Ok(Segment::Single);
    }
    if seg == "**" {
        return Ok(Segment::Rest);
    }
    if let Some(inner) = seg.strip_prefix('{') {
        let inner = inner
            .strip_suffix('}')
            .ok_or_else(|| ParseError::new(ParseErrorKind::UnbalancedBraces))?;
        return parse_variable(inner);
    }
    // Plain literal: reject stray braces and wildcard characters.
    if seg.contains('{') || seg.contains('}') {
        return Err(ParseError::new(ParseErrorKind::UnbalancedBraces));
    }
    if seg.contains('*') {
        return Err(ParseError::new(ParseErrorKind::InvalidLiteral));
    }
    Ok(Segment::Literal(seg.to_owned()))
}

fn parse_variable(inner: &str) -> Result<Segment, ParseError> {
    let (field_str, sub_str) = match inner.split_once('=') {
        Some((field, sub)) => (field, Some(sub)),
        None => (inner, None),
    };

    if field_str.is_empty() {
        return Err(ParseError::new(ParseErrorKind::EmptyFieldPath));
    }

    let mut field_path = Vec::new();
    for ident in field_str.split('.') {
        if !is_valid_ident(ident) {
            return Err(ParseError::new(ParseErrorKind::InvalidFieldName));
        }
        field_path.push(ident.to_owned());
    }

    let segments = match sub_str {
        None => vec![Segment::Single],
        Some(sub) => {
            if sub.is_empty() {
                return Err(ParseError::new(ParseErrorKind::EmptySegment));
            }
            let mut parsed = Vec::new();
            for seg in sub.split('/') {
                if seg.is_empty() {
                    return Err(ParseError::new(ParseErrorKind::EmptySegment));
                }
                if seg.contains('{') || seg.contains('}') {
                    return Err(ParseError::new(ParseErrorKind::NestedVariable));
                }
                let segment = match seg {
                    "*" => Segment::Single,
                    "**" => Segment::Rest,
                    other if other.contains('*') => {
                        return Err(ParseError::new(ParseErrorKind::InvalidLiteral));
                    }
                    other => Segment::Literal(other.to_owned()),
                };
                parsed.push(segment);
            }
            parsed
        }
    };

    Ok(Segment::Variable(Variable::new(field_path, segments)))
}

fn is_valid_ident(ident: &str) -> bool {
    let mut chars = ident.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a `Segment::Variable` for assertions.
    fn var(field: &[&str], segments: Vec<Segment>) -> Segment {
        Segment::Variable(Variable::new(field.iter().map(|s| (*s).to_owned()).collect(), segments))
    }

    #[test]
    fn parses_simple_literals() {
        let t = PathTemplate::parse("/v1/shelves").expect("valid");
        assert_eq!(t.segments(), &[Segment::Literal("v1".into()), Segment::Literal("shelves".into())]);
        assert!(t.verb().is_none());
    }

    #[test]
    fn parses_root() {
        let t = PathTemplate::parse("/").expect("root is valid");
        assert!(t.segments().is_empty());
        assert!(t.verb().is_none());
    }

    #[test]
    fn parses_single_wildcard_segment() {
        let t = PathTemplate::parse("/v1/*").expect("valid");
        assert_eq!(t.segments(), &[Segment::Literal("v1".into()), Segment::Single]);
    }

    #[test]
    fn parses_variable_shorthand_to_single() {
        let t = PathTemplate::parse("/v1/shelves/{shelf}").expect("valid");
        assert_eq!(
            t.segments(),
            &[
                Segment::Literal("v1".into()),
                Segment::Literal("shelves".into()),
                var(&["shelf"], vec![Segment::Single]),
            ]
        );
    }

    #[test]
    fn parses_nested_field_path() {
        let t = PathTemplate::parse("/v1/{shelf.id}").expect("valid");
        assert_eq!(
            t.segments(),
            &[Segment::Literal("v1".into()), var(&["shelf", "id"], vec![Segment::Single])]
        );
    }

    #[test]
    fn parses_variable_subtemplate_and_rest() {
        // Exercises the variable sub-template branch of `validate_rest_position`
        // on its accepted (trailing `**`) path.
        let t = PathTemplate::parse("/v1/{name=books/**}").expect("valid");
        assert_eq!(
            t.segments(),
            &[
                Segment::Literal("v1".into()),
                var(&["name"], vec![Segment::Literal("books".into()), Segment::Rest]),
            ]
        );
    }

    #[test]
    fn parses_trailing_rest() {
        let t = PathTemplate::parse("/v1/files/**").expect("valid");
        assert_eq!(t.segments().last(), Some(&Segment::Rest));
    }

    #[test]
    fn variable_accessors_return_field_and_subtemplate() {
        let v = Variable::new(vec!["shelf".to_owned(), "id".to_owned()], vec![Segment::Single]);
        assert_eq!(v.field_path(), &["shelf".to_owned(), "id".to_owned()]);
        assert_eq!(v.segments(), &[Segment::Single]);
    }

    #[test]
    fn parses_custom_verb() {
        let t = PathTemplate::parse("/v1/{name=shelves/*}:read").expect("valid");
        assert_eq!(t.verb(), Some("read"));
    }

    #[test]
    fn requires_leading_slash() {
        assert_eq!(PathTemplate::parse("v1/x").unwrap_err().kind(), ParseErrorKind::MissingLeadingSlash);
    }

    #[test]
    fn rejects_empty_segment() {
        assert_eq!(PathTemplate::parse("/a//b").unwrap_err().kind(), ParseErrorKind::EmptySegment);
    }

    #[test]
    fn rejects_rest_not_last() {
        assert_eq!(PathTemplate::parse("/a/**/b").unwrap_err().kind(), ParseErrorKind::RestNotLast);
    }

    #[test]
    fn rejects_rest_not_last_inside_variable() {
        // A `**` inside a variable sub-template that is not the final atom.
        assert_eq!(PathTemplate::parse("/a/{x=**}/b").unwrap_err().kind(), ParseErrorKind::RestNotLast);
    }

    #[test]
    fn rejects_nested_variable() {
        assert_eq!(
            PathTemplate::parse("/a/{x={y}}").unwrap_err().kind(),
            ParseErrorKind::NestedVariable
        );
    }

    #[test]
    fn rejects_invalid_field_name() {
        assert_eq!(
            PathTemplate::parse("/a/{1bad}").unwrap_err().kind(),
            ParseErrorKind::InvalidFieldName
        );
    }

    #[test]
    fn rejects_empty_verb() {
        assert_eq!(PathTemplate::parse("/a/b:").unwrap_err().kind(), ParseErrorKind::EmptyVerb);
    }

    #[test]
    fn rejects_multiple_verbs() {
        assert_eq!(PathTemplate::parse("/a:b:c").unwrap_err().kind(), ParseErrorKind::MultipleVerbs);
    }

    #[test]
    fn rejects_unbalanced_braces() {
        assert_eq!(PathTemplate::parse("/a/{x").unwrap_err().kind(), ParseErrorKind::UnbalancedBraces);
    }

    #[test]
    fn parse_error_display_covers_every_kind() {
        let cases = [
            ("no-leading-slash", ParseErrorKind::MissingLeadingSlash),
            ("/a//b", ParseErrorKind::EmptySegment),
            ("/a/{x", ParseErrorKind::UnbalancedBraces),
            ("/a/{x={y}}", ParseErrorKind::NestedVariable),
            ("/a/{=x}", ParseErrorKind::EmptyFieldPath),
            ("/a/{1bad}", ParseErrorKind::InvalidFieldName),
            ("/a*b", ParseErrorKind::InvalidLiteral),
            ("/a/**/b", ParseErrorKind::RestNotLast),
            ("/a/b:", ParseErrorKind::EmptyVerb),
            ("/a:b:c", ParseErrorKind::MultipleVerbs),
        ];
        for (template, kind) in cases {
            let err = PathTemplate::parse(template).unwrap_err();
            assert_eq!(err.kind(), kind, "kind for {template:?}");
            // Formatting exercises `ParseErrorKind::describe`.
            assert!(!err.to_string().is_empty(), "display for {template:?}");
        }
    }

    #[test]
    fn rejects_stray_brace_in_literal() {
        // Braces are balanced overall (so the depth scan passes) but the literal
        // segment `a{b}` still isn't a valid `{...}` variable.
        assert_eq!(PathTemplate::parse("/x/a{b}").unwrap_err().kind(), ParseErrorKind::UnbalancedBraces);
    }

    #[test]
    fn rejects_unbalanced_closing_brace() {
        assert_eq!(PathTemplate::parse("/a/}").unwrap_err().kind(), ParseErrorKind::UnbalancedBraces);
    }

    #[test]
    fn rejects_empty_subtemplate() {
        assert_eq!(PathTemplate::parse("/a/{x=}").unwrap_err().kind(), ParseErrorKind::EmptySegment);
    }

    #[test]
    fn rejects_empty_subtemplate_segment() {
        assert_eq!(PathTemplate::parse("/a/{x=a//b}").unwrap_err().kind(), ParseErrorKind::EmptySegment);
    }

    #[test]
    fn rejects_star_in_subtemplate_literal() {
        assert_eq!(
            PathTemplate::parse("/a/{x=a*b}").unwrap_err().kind(),
            ParseErrorKind::InvalidLiteral
        );
    }
}
