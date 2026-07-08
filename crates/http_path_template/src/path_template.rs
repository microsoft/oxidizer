// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::error::{ParseError, ParseErrorKind};
use crate::grammar::Grammar;
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
/// use http_path_template::{Grammar, PathTemplate};
///
/// # fn main() -> Result<(), http_path_template::ParseError> {
/// let template = PathTemplate::parse("/v1/shelves/{shelf}:get", Grammar::default())?;
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
    /// Parses a `google.api.http` path template string using the given [`Grammar`].
    ///
    /// Pass [`Grammar::default`] for the strict `google.api.http` syntax, or a
    /// [`Grammar`] with extensions enabled (e.g.
    /// [`Grammar::with_segment_affixes`]) to accept a superset of that syntax.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::{Grammar, PathTemplate, Segment};
    ///
    /// # fn main() -> Result<(), http_path_template::ParseError> {
    /// // The strict grammar.
    /// let template = PathTemplate::parse(
    ///     "/shelves/{shelf}/books/{book=**}:archive",
    ///     Grammar::default(),
    /// )?;
    /// assert_eq!(template.verb(), Some("archive"));
    /// assert_eq!(template.segments().len(), 4);
    ///
    /// // An intra-segment parameter needs the extended grammar. The strict
    /// // grammar has no such syntax, so it rejects this.
    /// assert!(PathTemplate::parse("/img-{id}.png", Grammar::default()).is_err());
    ///
    /// let template = PathTemplate::parse(
    ///     "/img-{id}.png",
    ///     Grammar::default().with_segment_affixes(),
    /// )?;
    /// assert_eq!(
    ///     template.segments()[0],
    ///     Segment::Affix {
    ///         prefix: "img-".to_owned(),
    ///         name: vec!["id".to_owned()],
    ///         suffix: ".png".to_owned(),
    ///     },
    /// );
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] describing the first structural problem found
    /// (missing leading slash, unbalanced braces, misplaced `**`, …).
    pub fn parse(template: impl AsRef<str>, grammar: Grammar) -> Result<Self, ParseError> {
        Self::parse_inner(template.as_ref(), grammar)
    }

    fn parse_inner(template: &str, grammar: Grammar) -> Result<Self, ParseError> {
        let (body, verb) = split_verb(template)?;

        let rest = body
            .strip_prefix('/')
            .ok_or_else(|| ParseError::new(ParseErrorKind::MissingLeadingSlash))?;

        let segments = if rest.is_empty() {
            Vec::new()
        } else {
            let raw = split_segments(rest)?;
            let mut parsed = Vec::with_capacity(raw.len());
            for seg in raw {
                parsed.push(parse_segment(seg, grammar.segment_affixes())?);
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
    /// use http_path_template::{Grammar, PathTemplate, Segment};
    ///
    /// # fn main() -> Result<(), http_path_template::ParseError> {
    /// let template = PathTemplate::parse("/v1/shelves/{shelf}", Grammar::default())?;
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
    /// use http_path_template::{Grammar, PathTemplate};
    ///
    /// # fn main() -> Result<(), http_path_template::ParseError> {
    /// let template = PathTemplate::parse("/v1/{name=shelves/*}:read", Grammar::default())?;
    /// assert_eq!(template.verb(), Some("read"));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn verb(&self) -> Option<&str> {
        self.verb.as_deref()
    }
}

impl core::str::FromStr for PathTemplate {
    type Err = ParseError;

    fn from_str(template: &str) -> Result<Self, Self::Err> {
        Self::parse(template, Grammar::default())
    }
}

impl core::fmt::Display for PathTemplate {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("/")?;
        for (idx, segment) in self.segments.iter().enumerate() {
            if idx != 0 {
                f.write_str("/")?;
            }
            write_segment(f, segment)?;
        }
        if let Some(verb) = &self.verb {
            write!(f, ":{verb}")?;
        }
        Ok(())
    }
}

/// Writes a single top-level segment in its template syntax.
fn write_segment(f: &mut core::fmt::Formatter<'_>, segment: &Segment) -> core::fmt::Result {
    match segment {
        Segment::Literal(text) => f.write_str(text),
        Segment::Single => f.write_str("*"),
        Segment::Rest => f.write_str("**"),
        Segment::Variable(variable) => write_variable(f, variable),
        Segment::Affix { prefix, name, suffix } => {
            f.write_str(prefix)?;
            f.write_str("{")?;
            for (idx, ident) in name.iter().enumerate() {
                if idx != 0 {
                    f.write_str(".")?;
                }
                f.write_str(ident)?;
            }
            f.write_str("}")?;
            f.write_str(suffix)
        }
    }
}

/// Writes a `{field.path=sub-template}` variable binding, collapsing a lone `*`
/// sub-template to the `{field}` shorthand.
fn write_variable(f: &mut core::fmt::Formatter<'_>, variable: &Variable) -> core::fmt::Result {
    f.write_str("{")?;
    for (idx, ident) in variable.field_path().iter().enumerate() {
        if idx != 0 {
            f.write_str(".")?;
        }
        f.write_str(ident)?;
    }
    if !matches!(variable.segments(), [Segment::Single]) {
        f.write_str("=")?;
        for (idx, sub) in variable.segments().iter().enumerate() {
            if idx != 0 {
                f.write_str("/")?;
            }
            write_segment(f, sub)?;
        }
    }
    f.write_str("}")
}

/// Validates that a `**` ([`Segment::Rest`]) appears only as the final atom of
/// the flattened template (including atoms contributed by variable
/// sub-templates), the only position a segment matcher can honor.
fn validate_rest_position(segments: &[Segment]) -> Result<(), ParseError> {
    // Collect, in flattened order, whether each atom is a `**`.
    let mut is_rest = Vec::new();
    for seg in segments {
        match seg {
            Segment::Literal(_) | Segment::Single | Segment::Affix { .. } => is_rest.push(false),
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
            // A verb is a `LITERAL`, so it may not contain the structural
            // characters `/`, `*`, `{`, or `}`. (Rejecting braces also prevents a
            // stray `{` in the verb from unbalancing the colon-depth scan above.)
            if verb.contains(['/', '*', '{', '}']) {
                return Err(ParseError::new(ParseErrorKind::InvalidVerb));
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

fn parse_segment(seg: &str, extended: bool) -> Result<Segment, ParseError> {
    if seg == "*" {
        return Ok(Segment::Single);
    }
    if seg == "**" {
        return Ok(Segment::Rest);
    }
    // A pure `{...}` variable occupies the whole segment.
    if seg.starts_with('{') && seg.ends_with('}') {
        return parse_variable(&seg[1..seg.len() - 1]);
    }
    // Extended grammar: a single `{field}` variable wrapped in literal text
    // (`prefix{field}suffix`).
    if extended && seg.contains('{') {
        return parse_affix(seg);
    }
    // Plain literal: reject stray braces and wildcard characters. A segment
    // reaching here always has balanced braces (`split_segments` guarantees it),
    // so a `{` implies an embedded, malformed brace.
    if seg.contains('{') {
        return Err(ParseError::new(ParseErrorKind::UnbalancedBraces));
    }
    if seg.contains('*') {
        return Err(ParseError::new(ParseErrorKind::InvalidLiteral));
    }
    Ok(Segment::Literal(seg.to_owned()))
}

/// Parses an extended-grammar `prefix{field.path}suffix` segment into a
/// [`Segment::Affix`]. Exactly one `{field}` variable is permitted, wrapped in
/// literal (wildcard-free) text; at least one of prefix/suffix is non-empty
/// (a bare `{field}` is handled as a plain variable before reaching here).
fn parse_affix(seg: &str) -> Result<Segment, ParseError> {
    // `split_segments` slices this segment between depth-0 `/`s, so its braces
    // are balanced (`{` and `}` occur in equal numbers). The total brace count
    // is therefore 2 exactly when there is one `{` and one `}`; anything else
    // (a nested `{...{...}...}`) is rejected. Summing keeps this a single
    // comparison rather than two provably-equal clauses joined by `||`.
    if seg.matches('{').count() + seg.matches('}').count() != 2 {
        return Err(ParseError::new(ParseErrorKind::UnbalancedBraces));
    }
    // `split_segments` already rejected any `}` that precedes its `{` (a closing
    // brace at brace-depth 0), so with exactly one of each the `{` always comes
    // first and `open < close` holds.
    let open = seg.find('{').expect("one '{' counted above");
    let close = seg.find('}').expect("one '}' counted, after the '{' by split_segments");

    let prefix = &seg[..open];
    let field_str = &seg[open + 1..close];
    let suffix = &seg[close + 1..];

    // Prefix and suffix are literals, so a `*` is invalid there.
    if prefix.contains('*') || suffix.contains('*') {
        return Err(ParseError::new(ParseErrorKind::InvalidLiteral));
    }
    if field_str.is_empty() {
        return Err(ParseError::new(ParseErrorKind::EmptyFieldPath));
    }
    // An affix variable is a simple dotted field path — no `=` sub-template.
    let mut name = Vec::new();
    for ident in field_str.split('.') {
        if !is_valid_ident(ident) {
            return Err(ParseError::new(ParseErrorKind::InvalidFieldName));
        }
        name.push(ident.to_owned());
    }

    Ok(Segment::Affix {
        prefix: prefix.to_owned(),
        name,
        suffix: suffix.to_owned(),
    })
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
                if seg.contains('{') {
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
