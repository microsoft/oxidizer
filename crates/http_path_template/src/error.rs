// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Parse errors ([`ParseError`] / [`ParseErrorKind`]).

use core::fmt;
use std::backtrace::Backtrace;

/// The kind of error produced while parsing a [`PathTemplate`].
///
/// # Examples
///
/// ```
/// use http_path_template::{ParseErrorKind, PathTemplate};
///
/// let error = PathTemplate::parse("shelves/{shelf}").expect_err("missing leading slash");
/// assert_eq!(error.kind(), ParseErrorKind::MissingLeadingSlash);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ParseErrorKind {
    /// The template did not begin with `/`.
    MissingLeadingSlash,
    /// The template contained an empty path segment (e.g. `a//b`).
    EmptySegment,
    /// A `{` or `}` was unbalanced.
    UnbalancedBraces,
    /// A variable sub-template contained a nested `{` variable.
    NestedVariable,
    /// A variable had an empty field path (e.g. `{}` or `{=*}`).
    EmptyFieldPath,
    /// A field-path identifier was empty or contained invalid characters.
    InvalidFieldName,
    /// A literal segment was empty or contained an invalid `*`.
    InvalidLiteral,
    /// A `**` appeared somewhere other than the final position.
    RestNotLast,
    /// A `:` verb separator was present but the verb was empty.
    EmptyVerb,
    /// More than one top-level `:` verb separator was present.
    MultipleVerbs,
}

impl ParseErrorKind {
    const fn describe(self) -> &'static str {
        match self {
            Self::MissingLeadingSlash => "template must begin with '/'",
            Self::EmptySegment => "template contains an empty path segment",
            Self::UnbalancedBraces => "template contains unbalanced '{' or '}'",
            Self::NestedVariable => "variable sub-templates may not contain nested variables",
            Self::EmptyFieldPath => "variable has an empty field path",
            Self::InvalidFieldName => "variable field path contains an invalid identifier",
            Self::InvalidLiteral => "literal segment is empty or contains a misplaced '*'",
            Self::RestNotLast => "'**' may only appear as the final segment",
            Self::EmptyVerb => "custom verb after ':' is empty",
            Self::MultipleVerbs => "template contains more than one ':' verb separator",
        }
    }
}

/// An error produced while parsing a [`PathTemplate`].
///
/// # Examples
///
/// ```
/// use http_path_template::PathTemplate;
///
/// let error = PathTemplate::parse("/a/**/b").expect_err("rest is not last");
/// assert_eq!(
///     error.to_string(),
///     "'**' may only appear as the final segment"
/// );
/// ```
#[derive(Debug)]
pub struct ParseError {
    kind: ParseErrorKind,
    #[expect(dead_code, reason = "captured for Debug output and RUST_BACKTRACE diagnostics")]
    backtrace: Box<Backtrace>,
}

impl ParseError {
    pub(crate) fn new(kind: ParseErrorKind) -> Self {
        Self {
            kind,
            backtrace: Box::new(Backtrace::capture()),
        }
    }

    /// The kind of parse failure that occurred.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::{ParseErrorKind, PathTemplate};
    ///
    /// let error = PathTemplate::parse("/a/{1bad}").expect_err("invalid field name");
    /// assert_eq!(error.kind(), ParseErrorKind::InvalidFieldName);
    /// ```
    #[must_use]
    pub const fn kind(&self) -> ParseErrorKind {
        self.kind
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind.describe())
    }
}

impl std::error::Error for ParseError {}
