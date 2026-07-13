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
pub struct PathTemplate<'a> {
    segments: Box<[Segment<'a>]>,
    verb: Option<&'a str>,
}

impl<'a> PathTemplate<'a> {
    /// Parses a `google.api.http` path template string using the given [`Grammar`].
    ///
    /// The returned template borrows from `template`: every literal, field name,
    /// and verb is a slice into the input, so parsing allocates only the
    /// top-level segment list.
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
    /// let template = PathTemplate::parse("/img-{id}.png", Grammar::default().with_segment_affixes())?;
    /// assert_eq!(
    ///     template.segments()[0],
    ///     Segment::Affix {
    ///         prefix: "img-",
    ///         name: "id",
    ///         suffix: ".png"
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
    pub fn parse(template: &'a str, grammar: Grammar) -> Result<Self, ParseError> {
        Self::parse_inner(template, grammar)
    }

    fn parse_inner(template: &'a str, grammar: Grammar) -> Result<Self, ParseError> {
        // The leading `/` is the most fundamental structural requirement, so
        // check it before the verb split (which could otherwise surface a later
        // error such as an unbalanced brace first).
        if template.as_bytes().first() != Some(&b'/') {
            return Err(ParseError::new(ParseErrorKind::MissingLeadingSlash));
        }

        let (body, verb) = split_verb(template)?;

        // `body` keeps the leading `/`: the verb split only trims a trailing
        // `:verb`, and the top-level `:` cannot be at index 0 given the check
        // above.
        let rest = &body[1..];

        let segments = if rest.is_empty() {
            Box::default()
        } else {
            split_and_parse_segments(rest, grammar.segment_affixes())?
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
    /// assert_eq!(template.segments()[0], Segment::Literal("v1"));
    /// assert!(matches!(template.segments()[2], Segment::Variable(_)));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn segments(&self) -> &[Segment<'a>] {
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
    pub fn verb(&self) -> Option<&'a str> {
        self.verb
    }
}

impl core::fmt::Display for PathTemplate<'_> {
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
fn write_segment(f: &mut core::fmt::Formatter<'_>, segment: &Segment<'_>) -> core::fmt::Result {
    match segment {
        Segment::Literal(text) => f.write_str(text),
        Segment::Single => f.write_str("*"),
        Segment::Rest => f.write_str("**"),
        Segment::Variable(variable) => write_variable(f, variable),
        Segment::Affix { prefix, name, suffix } => {
            f.write_str(prefix)?;
            f.write_str("{")?;
            f.write_str(name)?;
            f.write_str("}")?;
            f.write_str(suffix)
        }
    }
}

/// Writes a `{field.path=sub-template}` variable binding, collapsing a lone `*`
/// sub-template to the `{field}` shorthand.
fn write_variable(f: &mut core::fmt::Formatter<'_>, variable: &Variable<'_>) -> core::fmt::Result {
    f.write_str("{")?;
    f.write_str(variable.field_path())?;
    // `sub` is the exact sub-template substring (`*` for the shorthand), so it
    // renders verbatim. Collapse the lone-`*` shorthand back to `{field}`.
    let sub = variable.sub();
    if sub != "*" {
        f.write_str("=")?;
        f.write_str(sub)?;
    }
    f.write_str("}")
}

/// Validates that a `**` ([`Segment::Rest`]) appears only as the final atom of
/// the flattened template (including atoms contributed by variable
/// sub-templates), the only position a segment matcher can honor.
fn validate_rest_position(segments: &[Segment]) -> Result<(), ParseError> {
    // Walk atoms in flattened order. Once a `**` (`Rest`) atom is seen, no
    // further atom may follow it: any atom reached while `seen_rest` is set means
    // an earlier `**` was not last.
    let mut seen_rest = false;
    for seg in segments {
        match seg {
            Segment::Literal(_) | Segment::Single | Segment::Affix { .. } => {
                if seen_rest {
                    return Err(ParseError::new(ParseErrorKind::RestNotLast));
                }
            }
            Segment::Rest => {
                if seen_rest {
                    return Err(ParseError::new(ParseErrorKind::RestNotLast));
                }
                seen_rest = true;
            }
            Segment::Variable(var) => {
                for sub in var.segments() {
                    if seen_rest {
                        return Err(ParseError::new(ParseErrorKind::RestNotLast));
                    }
                    if matches!(sub, Segment::Rest) {
                        seen_rest = true;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Splits a template into its body and optional verb at the first top-level `:`.
fn split_verb(template: &str) -> Result<(&str, Option<&str>), ParseError> {
    // The characters that matter here (`{`, `}`, `:`) are ASCII, so scan bytes.
    let mut depth = 0_usize;
    let mut verb_at = None;
    for (idx, &b) in template.as_bytes().iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' if depth > 0 => depth -= 1,
            // A `}` with no matching `{` before the verb separator is the first
            // structural error, so report it here rather than letting later verb
            // validation win. After the verb separator such a `}` is part of the
            // (invalid) verb, which the verb check below rejects.
            b'}' if verb_at.is_none() => {
                return Err(ParseError::new(ParseErrorKind::UnbalancedBraces));
            }
            b':' if depth == 0 => {
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
            Ok((&template[..idx], Some(verb)))
        }
        None => Ok((template, None)),
    }
}

/// Splits a template body (after the leading `/`) into segments, parsing each
/// one into the returned [`Segment`] vector and treating `/` inside `{...}` as
/// literal.
///
/// All structural delimiters (`/`, `{`, `}`) are ASCII, so the byte offsets used
/// to slice `body` always land on UTF-8 boundaries.
fn split_and_parse_segments(body: &str, extended: bool) -> Result<Box<[Segment<'_>]>, ParseError> {
    // Pre-size to the segment count so pushing never reallocates (a hint only;
    // see `segment_count_hint`).
    let mut out = Vec::with_capacity(segment_count_hint(body));

    let mut depth = 0_usize;
    let mut start = 0_usize;
    for (idx, &b) in body.as_bytes().iter().enumerate() {
        match b {
            b'{' => depth += 1,
            // `split_verb` already rejected any `}` with no matching `{`, so the
            // body reaching here has no stray closing brace and `depth` never
            // underflows. An unclosed `{` is caught by the `depth != 0` check
            // after the loop.
            b'}' => depth -= 1,
            b'/' if depth == 0 => {
                push_parsed_segment(&mut out, &body[start..idx], extended)?;
                start = idx + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(ParseError::new(ParseErrorKind::UnbalancedBraces));
    }
    push_parsed_segment(&mut out, &body[start..], extended)?;

    Ok(out.into_boxed_slice())
}

/// Counts the depth-0 segments in `body` to pre-size the output vector. An
/// incorrect count only affects whether that vector reallocates, never the
/// parsed result.
#[cfg_attr(test, mutants::skip)]
fn segment_count_hint(body: &str) -> usize {
    let mut depth = 0_usize;
    let mut count = 1_usize;
    for &b in body.as_bytes() {
        match b {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b'/' if depth == 0 => count += 1,
            _ => {}
        }
    }
    count
}

/// Parses one raw segment slice and appends it to `out`, rejecting an empty
/// segment (e.g. from `a//b`).
fn push_parsed_segment<'a>(out: &mut Vec<Segment<'a>>, seg: &'a str, extended: bool) -> Result<(), ParseError> {
    if seg.is_empty() {
        return Err(ParseError::new(ParseErrorKind::EmptySegment));
    }
    out.push(parse_segment(seg, extended)?);
    Ok(())
}

fn parse_segment(seg: &str, extended: bool) -> Result<Segment<'_>, ParseError> {
    if seg == "*" {
        return Ok(Segment::Single);
    }
    if seg == "**" {
        return Ok(Segment::Rest);
    }
    let bytes = seg.as_bytes();
    // A pure `{...}` variable occupies the whole segment.
    if bytes.first() == Some(&b'{') && bytes.last() == Some(&b'}') {
        return parse_variable(&seg[1..seg.len() - 1]);
    }
    // Extended grammar: a single `{field}` variable wrapped in literal text
    // (`prefix{field}suffix`).
    if extended && bytes.contains(&b'{') {
        return parse_affix(seg);
    }
    // Plain literal: reject stray braces and wildcard characters. A segment
    // reaching here always has balanced braces (`split_and_parse_segments`
    // guarantees it), so a `{` implies an embedded, malformed brace. Braces take
    // priority over `*`, so return immediately on a `{` (it wins wherever it
    // appears) and only flag a `*` for after the scan.
    let mut has_star = false;
    for &byte in seg.as_bytes() {
        match byte {
            b'{' => return Err(ParseError::new(ParseErrorKind::UnbalancedBraces)),
            b'*' => has_star = true,
            _ => {}
        }
    }
    if has_star {
        return Err(ParseError::new(ParseErrorKind::InvalidLiteral));
    }
    Ok(Segment::Literal(seg))
}

/// Parses an extended-grammar `prefix{field.path}suffix` segment into a
/// [`Segment::Affix`]. Exactly one `{field}` variable is permitted, wrapped in
/// literal (wildcard-free) text; at least one of prefix/suffix is non-empty
/// (a bare `{field}` is handled as a plain variable before reaching here).
fn parse_affix(seg: &str) -> Result<Segment<'_>, ParseError> {
    // `split_and_parse_segments` slices this segment between depth-0 `/`s, so its
    // braces are balanced. Record the first `{`/`}` positions and the total brace
    // count: exactly one of each (a total of 2) is required, so a different count
    // means a nested `{...{...}...}` and is rejected.
    let mut open = None;
    let mut close = None;
    let mut braces = 0_usize;
    for (idx, &byte) in seg.as_bytes().iter().enumerate() {
        match byte {
            b'{' => {
                braces += 1;
                if open.is_none() {
                    open = Some(idx);
                }
            }
            b'}' => {
                braces += 1;
                if close.is_none() {
                    close = Some(idx);
                }
            }
            _ => {}
        }
    }
    if braces != 2 {
        return Err(ParseError::new(ParseErrorKind::UnbalancedBraces));
    }
    // With exactly one `{` and one `}`: `split_verb` already rejected any `}`
    // with no matching `{` (a closing brace at brace-depth 0) during the
    // top-level scan, so the open brace always comes first and `open < close`
    // holds.
    let open = open.expect("one '{' counted above");
    let close = close.expect("one '}' counted above");

    let prefix = &seg[..open];
    let field_str = &seg[open + 1..close];
    let suffix = &seg[close + 1..];

    // Prefix and suffix are literals, so a `*` is invalid there.
    if prefix.as_bytes().contains(&b'*') || suffix.as_bytes().contains(&b'*') {
        return Err(ParseError::new(ParseErrorKind::InvalidLiteral));
    }
    if field_str.is_empty() {
        return Err(ParseError::new(ParseErrorKind::EmptyFieldPath));
    }
    // An affix variable is a simple dotted field path — no `=` sub-template.
    // Validate every identifier; the raw dotted string is stored as-is.
    for ident in field_str.as_bytes().split(|&b| b == b'.') {
        if !is_valid_ident(ident) {
            return Err(ParseError::new(ParseErrorKind::InvalidFieldName));
        }
    }

    Ok(Segment::Affix {
        prefix,
        name: field_str,
        suffix,
    })
}

fn parse_variable(inner: &str) -> Result<Segment<'_>, ParseError> {
    // Delimiters (`=`, `.`, `/`) are ASCII, so split and scan on bytes to avoid
    // the UTF-8-aware char searcher. Byte offsets at ASCII delimiters are valid
    // `str` boundaries.
    let (field_str, sub_str) = match inner.as_bytes().iter().position(|&b| b == b'=') {
        Some(idx) => (&inner[..idx], Some(&inner[idx + 1..])),
        None => (inner, None),
    };

    if field_str.is_empty() {
        return Err(ParseError::new(ParseErrorKind::EmptyFieldPath));
    }

    // Validate every field-path identifier; the raw dotted string is stored as-is.
    for ident in field_str.as_bytes().split(|&b| b == b'.') {
        if !is_valid_ident(ident) {
            return Err(ParseError::new(ParseErrorKind::InvalidFieldName));
        }
    }

    // The sub-template is stored as its raw substring and re-split lazily by
    // `Variable::segments`. Validate it here so that iteration is infallible. The
    // `{field}` shorthand normalizes to `*` (equal to `{field=*}`), which lets
    // the borrowed `Variable` derive `PartialEq`/`Hash`.
    let sub = match sub_str {
        None => "*",
        Some(sub) => {
            if sub.is_empty() {
                return Err(ParseError::new(ParseErrorKind::EmptySegment));
            }
            for seg in sub.as_bytes().split(|&b| b == b'/') {
                if seg.is_empty() {
                    return Err(ParseError::new(ParseErrorKind::EmptySegment));
                }
                if seg.contains(&b'{') {
                    return Err(ParseError::new(ParseErrorKind::NestedVariable));
                }
                // Only `*`/`**` may contain a wildcard; any other `*` is invalid.
                if seg != b"*" && seg != b"**" && seg.contains(&b'*') {
                    return Err(ParseError::new(ParseErrorKind::InvalidLiteral));
                }
            }
            sub
        }
    };

    Ok(Segment::Variable(Variable::new(field_str, sub)))
}

fn is_valid_ident(ident: &[u8]) -> bool {
    // Identifiers are ASCII; a non-ASCII lead byte fails the `is_ascii_*` checks
    // and is rejected.
    match ident.first() {
        Some(&b) if b == b'_' || b.is_ascii_alphabetic() => {}
        _ => return false,
    }
    ident[1..].iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric())
}
