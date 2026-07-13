// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;
use std::backtrace::{Backtrace, BacktraceStatus};

/// The specific structural failure behind a [`ParseError`]. Kept crate-internal;
/// callers discriminate failures through the `ParseError::is_*` predicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ParseErrorKind {
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
    /// A literal segment contained a misplaced `*` (`*`/`**` are only valid as
    /// whole segments).
    InvalidLiteral,
    /// A `**` appeared somewhere other than the final position.
    RestNotLast,
    /// A `:` verb separator was present but the verb was empty.
    EmptyVerb,
    /// A `:` verb contained a `/`, `*`, `{`, or `}` (a verb must be a single
    /// trailing literal).
    InvalidVerb,
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
            Self::InvalidLiteral => "literal segment contains a misplaced '*'",
            Self::RestNotLast => "'**' may only appear as the final segment",
            Self::EmptyVerb => "custom verb after ':' is empty",
            Self::InvalidVerb => "custom verb after ':' must be a literal (no '/', '*', '{', or '}')",
            Self::MultipleVerbs => "template contains more than one ':' verb separator",
        }
    }
}

/// A lazily-allocated backtrace for a [`ParseError`].
///
/// A [`Backtrace`] is only allocated (boxed) when it is actually *captured* —
/// i.e. when `RUST_BACKTRACE` (or `RUST_LIB_BACKTRACE`) is enabled. In the common
/// case where backtraces are disabled, `Backtrace::capture()` returns a disabled
/// backtrace that carries no data, so we store [`MaybeBacktrace::Disabled`] and
/// avoid a heap allocation entirely. This keeps the error path allocation-free by
/// default while still surfacing a full backtrace in `Debug` when enabled.
#[derive(Debug)]
enum MaybeBacktrace {
    /// A captured backtrace, boxed to keep [`ParseError`] small. Only produced
    /// when backtrace capture is enabled.
    Captured(#[expect(dead_code, reason = "captured for Debug output and RUST_BACKTRACE diagnostics")] Box<Backtrace>),
    /// Backtrace capture was disabled or unsupported, so no allocation was made.
    Disabled,
}

impl MaybeBacktrace {
    /// Captures a backtrace, allocating only if capture is actually enabled.
    fn capture() -> Self {
        Self::from_backtrace(Backtrace::capture())
    }

    /// Wraps a [`Backtrace`], boxing it only when it actually captured frames.
    fn from_backtrace(backtrace: Backtrace) -> Self {
        match backtrace.status() {
            BacktraceStatus::Captured => Self::Captured(Box::new(backtrace)),
            // `Disabled`/`Unsupported` backtraces hold no frames, so there is
            // nothing worth allocating for.
            _ => Self::Disabled,
        }
    }

    /// Unconditionally captures a backtrace, ignoring the `RUST_BACKTRACE`
    /// setting. Used only in tests to exercise the [`MaybeBacktrace::Captured`]
    /// path regardless of the environment.
    #[cfg(test)]
    fn force_capture() -> Self {
        Self::from_backtrace(Backtrace::force_capture())
    }
}

/// An error produced while parsing a [`PathTemplate`](crate::PathTemplate).
///
/// # Examples
///
/// ```
/// use http_path_template::{Grammar, PathTemplate};
///
/// let error = PathTemplate::parse("/a/**/b", Grammar::default()).expect_err("rest is not last");
/// assert_eq!(
///     error.to_string(),
///     "'**' may only appear as the final segment"
/// );
/// ```
#[derive(Debug)]
pub struct ParseError {
    kind: ParseErrorKind,
    #[expect(dead_code, reason = "captured for Debug output and RUST_BACKTRACE diagnostics")]
    backtrace: MaybeBacktrace,
}

impl ParseError {
    pub(crate) fn new(kind: ParseErrorKind) -> Self {
        Self {
            kind,
            backtrace: MaybeBacktrace::capture(),
        }
    }

    /// Whether the template did not begin with `/`.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_path_template::{Grammar, PathTemplate};
    ///
    /// let error = PathTemplate::parse("shelves/{shelf}", Grammar::default())
    ///     .expect_err("missing leading slash");
    /// assert!(error.is_missing_leading_slash());
    /// ```
    #[must_use]
    pub const fn is_missing_leading_slash(&self) -> bool {
        matches!(self.kind, ParseErrorKind::MissingLeadingSlash)
    }

    /// Whether the template contained an empty path segment (e.g. `a//b`).
    #[must_use]
    pub const fn is_empty_segment(&self) -> bool {
        matches!(self.kind, ParseErrorKind::EmptySegment)
    }

    /// Whether a `{` or `}` was unbalanced.
    #[must_use]
    pub const fn is_unbalanced_braces(&self) -> bool {
        matches!(self.kind, ParseErrorKind::UnbalancedBraces)
    }

    /// Whether a variable sub-template contained a nested `{` variable.
    #[must_use]
    pub const fn is_nested_variable(&self) -> bool {
        matches!(self.kind, ParseErrorKind::NestedVariable)
    }

    /// Whether a variable had an empty field path (e.g. `{}` or `{=*}`).
    #[must_use]
    pub const fn is_empty_field_path(&self) -> bool {
        matches!(self.kind, ParseErrorKind::EmptyFieldPath)
    }

    /// Whether a field-path identifier was empty or contained invalid characters.
    #[must_use]
    pub const fn is_invalid_field_name(&self) -> bool {
        matches!(self.kind, ParseErrorKind::InvalidFieldName)
    }

    /// Whether a literal segment contained a misplaced `*`.
    #[must_use]
    pub const fn is_invalid_literal(&self) -> bool {
        matches!(self.kind, ParseErrorKind::InvalidLiteral)
    }

    /// Whether a `**` appeared somewhere other than the final position.
    #[must_use]
    pub const fn is_rest_not_last(&self) -> bool {
        matches!(self.kind, ParseErrorKind::RestNotLast)
    }

    /// Whether a `:` verb separator was present but the verb was empty.
    #[must_use]
    pub const fn is_empty_verb(&self) -> bool {
        matches!(self.kind, ParseErrorKind::EmptyVerb)
    }

    /// Whether a `:` verb contained a disallowed character (`/`, `*`, `{`, `}`).
    #[must_use]
    pub const fn is_invalid_verb(&self) -> bool {
        matches!(self.kind, ParseErrorKind::InvalidVerb)
    }

    /// Whether more than one top-level `:` verb separator was present.
    #[must_use]
    pub const fn is_multiple_verbs(&self) -> bool {
        matches!(self.kind, ParseErrorKind::MultipleVerbs)
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind.describe())
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Debug-formatting a captured Backtrace symbolicates frames via getcwd, unsupported under Miri isolation"
    )]
    fn force_capture_produces_a_captured_backtrace() {
        // `force_capture` ignores `RUST_BACKTRACE`, so this exercises the
        // `Captured` arm even when backtraces are disabled in the environment.
        let backtrace = MaybeBacktrace::force_capture();
        assert!(matches!(backtrace, MaybeBacktrace::Captured(_)));
        // The captured backtrace surfaces in `Debug` output.
        assert!(format!("{backtrace:?}").starts_with("Captured"));
    }

    #[test]
    fn from_backtrace_stores_no_allocation_when_disabled() {
        // A disabled backtrace holds no frames, so `from_backtrace` takes the
        // `Disabled` arm and avoids boxing anything.
        let backtrace = MaybeBacktrace::from_backtrace(Backtrace::disabled());
        assert!(matches!(backtrace, MaybeBacktrace::Disabled));
    }
}
