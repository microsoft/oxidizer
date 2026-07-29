// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::variable::Variable;

/// A single element of a parsed [`PathTemplate`](crate::PathTemplate).
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate, Segment};
///
/// # fn main() -> Result<(), http_path_template::ParseError> {
/// let template = PathTemplate::parse("/v1/shelves/*", Grammar::default())?;
/// assert_eq!(template.segments()[0], Segment::Literal("v1"));
/// assert!(matches!(template.segments()[2], Segment::Single));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Segment<'a> {
    /// A path-template literal that must match verbatim, e.g. `shelves`.
    /// It contains RFC 3986 `pchar` characters except raw `*`, which is reserved
    /// for wildcard atoms. Any percent encoding must use valid `%HH` escapes.
    Literal(&'a str),
    /// `*` — matches exactly one (non-empty) path segment.
    Single,
    /// `**` — matches the remaining path segments. Only valid as the final atom
    /// of the entire flattened template; a variable sub-template ending in `**`
    /// must therefore also be the final top-level segment.
    Rest,
    /// `{field.path=sub}` — a variable binding capturing the portion of the
    /// path matched by its sub-template.
    Variable(Variable<'a>),
    /// `<prefix>{field.path}<suffix>` — an *extended-grammar* single segment that
    /// matches a literal `prefix` and `suffix` around a captured middle (at least
    /// one of `prefix`/`suffix` is non-empty). Only produced when parsing with a
    /// [`Grammar`](crate::Grammar) that has
    /// [`with_segment_affixes`](crate::Grammar::with_segment_affixes) enabled; the
    /// strict `google.api.http` grammar rejects it.
    Affix {
        /// The literal text the segment must start with (may be empty).
        prefix: &'a str,
        /// The captured middle's dotted field path (e.g. `shelf.id`); split on
        /// `.` for the individual identifiers.
        name: &'a str,
        /// The literal text the segment must end with (may be empty).
        suffix: &'a str,
    },
}
