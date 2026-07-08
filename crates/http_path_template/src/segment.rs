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
/// assert!(matches!(template.segments()[0], Segment::Literal(ref text) if text == "v1"));
/// assert!(matches!(template.segments()[2], Segment::Single));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Segment {
    /// A literal path segment that must match verbatim, e.g. `shelves`.
    Literal(String),
    /// `*` — matches exactly one (non-empty) path segment.
    Single,
    /// `**` — matches the remaining path segments (zero or more). Only valid as
    /// the final element of a template or variable sub-template.
    Rest,
    /// `{field.path=sub}` — a variable binding capturing the portion of the
    /// path matched by its sub-template.
    Variable(Variable),
    /// `<prefix>{field.path}<suffix>` — an *extended-grammar* single segment that
    /// matches a literal `prefix` and `suffix` around a captured middle (at least
    /// one of `prefix`/`suffix` is non-empty). Only produced when parsing with a
    /// [`Grammar`](crate::Grammar) that has
    /// [`with_segment_affixes`](crate::Grammar::with_segment_affixes) enabled; the
    /// strict `google.api.http` grammar rejects it.
    Affix {
        /// The literal text the segment must start with (may be empty).
        prefix: String,
        /// The dotted field path the captured middle binds into.
        name: Vec<String>,
        /// The literal text the segment must end with (may be empty).
        suffix: String,
    },
}
