// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Segment`] element of a parsed path template.

use crate::variable::Variable;

/// A single element of a parsed [`PathTemplate`].
///
/// # Examples
///
/// ```
/// use http_path_template::{PathTemplate, Segment};
///
/// # fn main() -> Result<(), http_path_template::ParseError> {
/// let template = PathTemplate::parse("/v1/shelves/*")?;
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
}
