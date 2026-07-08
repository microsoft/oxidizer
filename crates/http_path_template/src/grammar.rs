// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Selects which path-template grammar [`PathTemplate::parse`](crate::PathTemplate::parse)
/// accepts.
///
/// The [`Default`] grammar is the strict `google.api.http` syntax. Non-standard
/// extensions are opted into explicitly with the `with_*` methods, so new
/// extensions can be added over time without breaking existing callers.
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate};
///
/// # fn main() -> Result<(), http_path_template::ParseError> {
/// // The default grammar is the strict `google.api.http` syntax.
/// let strict = Grammar::default();
/// assert!(PathTemplate::parse("/img-{id}.png", strict).is_err());
///
/// // Opt into intra-segment prefix/suffix parameters.
/// let extended = Grammar::default().with_segment_affixes();
/// assert!(PathTemplate::parse("/img-{id}.png", extended).is_ok());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Grammar {
    segment_affixes: bool,
}

impl Grammar {
    /// The strict `google.api.http` grammar, equivalent to [`Grammar::default`].
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::Grammar;
    ///
    /// assert_eq!(Grammar::new(), Grammar::default());
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self { segment_affixes: false }
    }

    /// Enables **intra-segment prefix/suffix parameters**: a single segment may
    /// wrap one `{field.path}` variable in literal text, for example
    /// `/files/{name}.json`, `/v{version}/x`, or `/img-{id}.png`. Such a segment
    /// parses to a [`Segment::Affix`](crate::Segment::Affix).
    ///
    /// This is a non-standard superset of the `google.api.http` grammar; the
    /// strict grammar rejects intra-segment parameters.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::{Grammar, PathTemplate, Segment};
    ///
    /// # fn main() -> Result<(), http_path_template::ParseError> {
    /// let grammar = Grammar::default().with_segment_affixes();
    /// let template = PathTemplate::parse("/img-{id}.png", grammar)?;
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
    #[must_use]
    pub const fn with_segment_affixes(mut self) -> Self {
        self.segment_affixes = true;
        self
    }

    /// Whether intra-segment prefix/suffix parameters are allowed.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::Grammar;
    ///
    /// assert!(!Grammar::default().segment_affixes());
    /// assert!(Grammar::default().with_segment_affixes().segment_affixes());
    /// ```
    #[must_use]
    pub const fn segment_affixes(self) -> bool {
        self.segment_affixes
    }
}
