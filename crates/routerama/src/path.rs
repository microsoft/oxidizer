// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Explicit path preparation that runs before route matching.
//!
//! Route matching is exact: it runs on the raw request path and normalizes
//! nothing, as documented under the matching semantics on [`crate::resolve`].
//! That is the right default, because a path segment is data as often as it is
//! structure, but it means the path this crate matches can differ from the path
//! a fronting proxy inspected. When the proxy normalizes `/admin/../public/x`
//! to `/public/x`, authorizes that, and forwards the original bytes, the two
//! views disagree and the proxy's decision no longer describes the route that
//! runs.
//!
//! [`PreparedPath`] closes that gap without changing how matching works. The
//! caller decides, through a [`PathPolicy`], which spellings are acceptable and
//! which are rewritten; the result reports whether it differs from the input so
//! a service can redirect to the canonical form rather than silently serving a
//! non-canonical one.
//!
//! # Ownership
//!
//! A resolved route borrows its captures out of the path it matched, so the
//! prepared value has to outlive the match. [`PreparedPath`] therefore holds
//! the path itself — borrowing the input when preparation changed nothing and
//! owning a rewritten string otherwise — and the caller keeps it alive for as
//! long as the resolved route is used.
//!
//! # Examples
//!
//! Canonicalize and redirect rather than serving a non-canonical path:
//!
//! ```
//! use routerama::path::{PathPolicy, PreparedPath};
//!
//! let prepared = PreparedPath::new("/store//items/../cart/", PathPolicy::STRICT)?;
//!
//! assert!(prepared.was_changed());
//! assert_eq!(prepared.as_str(), "/store/cart");
//! # Ok::<(), routerama::path::PathError>(())
//! ```
//!
//! Preserve every spelling, which is exactly what matching does on its own:
//!
//! ```
//! use routerama::path::{PathPolicy, PreparedPath};
//!
//! let prepared = PreparedPath::new("/store//items/../cart/", PathPolicy::EXACT)?;
//!
//! assert!(!prepared.was_changed());
//! assert_eq!(prepared.as_str(), "/store//items/../cart/");
//! # Ok::<(), routerama::path::PathError>(())
//! ```

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::decode::decode;

/// How to treat two or more consecutive separators.
///
/// A run of separators produces an empty segment, which no template can declare
/// and which only a `**` catch-all can consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepeatedSlashes {
    /// Keep the empty segments, matching what the resolver sees today.
    Preserve,
    /// Reject the path with [`PathError::RepeatedSlashes`].
    Reject,
    /// Reduce each run of separators to a single separator.
    Collapse,
}

/// How to treat the dot segments `.` and `..`.
///
/// This applies to *literal* dot segments only. A percent-encoded dot segment
/// such as `%2e%2e` is not decoded here, because decoding must never turn data
/// into routing structure; use [`EncodedSeparators::Reject`] to refuse those.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DotSegments {
    /// Keep `.` and `..` as ordinary segments, matching what the resolver sees
    /// today.
    Preserve,
    /// Reject the path with [`PathError::DotSegment`].
    Reject,
    /// Resolve them as RFC 3986 section 5.2.4 does, discarding any `..` that
    /// would escape above the root.
    Remove,
}

/// How to treat a trailing separator on a path other than the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrailingSlash {
    /// Keep the trailing separator, matching what the resolver sees today.
    Preserve,
    /// Reject the path with [`PathError::TrailingSlash`].
    Reject,
    /// Drop the trailing separator.
    Remove,
}

/// How to treat percent escapes that would decode into routing structure.
///
/// These are never rewritten, only accepted or refused: decoding `%2F` into a
/// separator would create structure the sender did not send, which is the
/// confusion this module exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncodedSeparators {
    /// Keep them, matching what the resolver sees today. They stay encoded
    /// during matching and are decoded only when a capture is read.
    Preserve,
    /// Reject the path with [`PathError::EncodedSeparator`].
    ///
    /// This refuses `%2F` and `%5C` anywhere in the path, and refuses any
    /// segment that would decode to `.` or `..`. It does not refuse an encoded
    /// dot inside a longer segment, so `file%2Etxt` is still accepted.
    Reject,
}

/// The spellings a [`PreparedPath`] accepts, and what it rewrites.
///
/// Two presets cover the common cases: [`PathPolicy::EXACT`] changes and
/// refuses nothing, and [`PathPolicy::STRICT`] canonicalizes what can be
/// canonicalized and refuses what cannot. Start from a preset and adjust one
/// axis at a time for anything else.
///
/// Regardless of policy, a path carrying a query (`?`) or fragment (`#`)
/// delimiter or a malformed percent escape is always rejected, because neither
/// is a spelling of a path that preparation could canonicalize.
///
/// # Examples
///
/// ```
/// use routerama::path::{DotSegments, PathPolicy};
///
/// let policy = PathPolicy::EXACT.with_dot_segments(DotSegments::Reject);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct PathPolicy {
    /// How to treat two or more consecutive separators.
    pub repeated_slashes: RepeatedSlashes,
    /// How to treat the dot segments `.` and `..`.
    pub dot_segments: DotSegments,
    /// How to treat a trailing separator on a path other than the root.
    pub trailing_slash: TrailingSlash,
    /// How to treat percent escapes that would decode into routing structure.
    pub encoded_separators: EncodedSeparators,
}

impl PathPolicy {
    /// Preserves every spelling, so preparation never rewrites the path.
    ///
    /// This is the behavior of route matching on its own. It still rejects a
    /// query or fragment delimiter and a malformed percent escape.
    pub const EXACT: Self = Self {
        repeated_slashes: RepeatedSlashes::Preserve,
        dot_segments: DotSegments::Preserve,
        trailing_slash: TrailingSlash::Preserve,
        encoded_separators: EncodedSeparators::Preserve,
    };

    /// Canonicalizes what can be canonicalized and rejects what cannot.
    ///
    /// Separator runs collapse, dot segments resolve, a trailing separator is
    /// dropped, and a percent escape that would decode into routing structure
    /// is refused rather than rewritten. A service that compares
    /// [`PreparedPath::was_changed`] against its input can redirect to the
    /// canonical spelling instead of serving several spellings of one route.
    pub const STRICT: Self = Self {
        repeated_slashes: RepeatedSlashes::Collapse,
        dot_segments: DotSegments::Remove,
        trailing_slash: TrailingSlash::Remove,
        encoded_separators: EncodedSeparators::Reject,
    };

    /// Returns this policy with [`Self::repeated_slashes`] replaced.
    #[must_use]
    pub const fn with_repeated_slashes(mut self, repeated_slashes: RepeatedSlashes) -> Self {
        self.repeated_slashes = repeated_slashes;
        self
    }

    /// Returns this policy with [`Self::dot_segments`] replaced.
    #[must_use]
    pub const fn with_dot_segments(mut self, dot_segments: DotSegments) -> Self {
        self.dot_segments = dot_segments;
        self
    }

    /// Returns this policy with [`Self::trailing_slash`] replaced.
    #[must_use]
    pub const fn with_trailing_slash(mut self, trailing_slash: TrailingSlash) -> Self {
        self.trailing_slash = trailing_slash;
        self
    }

    /// Returns this policy with [`Self::encoded_separators`] replaced.
    #[must_use]
    pub const fn with_encoded_separators(mut self, encoded_separators: EncodedSeparators) -> Self {
        self.encoded_separators = encoded_separators;
        self
    }
}

impl Default for PathPolicy {
    /// Returns [`PathPolicy::EXACT`], the policy that matches resolver behavior.
    fn default() -> Self {
        Self::EXACT
    }
}

/// An error returned when a request path cannot be prepared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PathError {
    /// The input contained a query (`?`) or fragment (`#`) delimiter and was
    /// therefore not a URI path.
    QueryOrFragment,

    /// The input contained a `%` that was not followed by two hexadecimal
    /// digits.
    MalformedEscape,

    /// The input contained two or more consecutive separators and the policy
    /// is [`RepeatedSlashes::Reject`].
    RepeatedSlashes,

    /// The input contained a `.` or `..` segment and the policy is
    /// [`DotSegments::Reject`].
    DotSegment,

    /// The input ended with a separator and the policy is
    /// [`TrailingSlash::Reject`].
    TrailingSlash,

    /// The input contained a percent escape that would decode into routing
    /// structure and the policy is [`EncodedSeparators::Reject`].
    EncodedSeparator,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::QueryOrFragment => "expected a URI path without a query or fragment delimiter",
            Self::MalformedEscape => "expected every `%` to be followed by two hexadecimal digits",
            Self::RepeatedSlashes => "expected no repeated `/` separators",
            Self::DotSegment => "expected no `.` or `..` path segment",
            Self::TrailingSlash => "expected no trailing `/` separator",
            Self::EncodedSeparator => "expected no percent escape that would decode into a path separator or dot segment",
        };
        f.write_str(message)
    }
}

impl core::error::Error for PathError {}

/// A request path that satisfies a [`PathPolicy`].
///
/// Borrows the input when preparation changed nothing, and owns a rewritten
/// string otherwise. Keep it alive for as long as any route resolved from it,
/// because captures borrow out of the path.
///
/// # Examples
///
/// ```
/// use routerama::path::{PathPolicy, PreparedPath};
///
/// let prepared = PreparedPath::new("/a/./b", PathPolicy::STRICT)?;
/// assert_eq!(prepared.as_str(), "/a/b");
/// assert!(prepared.was_changed());
/// # Ok::<(), routerama::path::PathError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreparedPath<'p> {
    value: Cow<'p, str>,
    changed: bool,
}

impl<'p> PreparedPath<'p> {
    /// Prepares `path` under `policy`.
    ///
    /// # Errors
    ///
    /// Returns [`PathError`] when the path carries a query or fragment
    /// delimiter, contains a malformed percent escape, or uses a spelling the
    /// policy rejects.
    pub fn new(path: &'p str, policy: PathPolicy) -> Result<Self, PathError> {
        validate(path, policy)?;

        let leading_slash = path.starts_with('/');
        let body = if leading_slash { &path[1..] } else { path };

        let Some(rewritten) = rewrite(body, leading_slash, policy) else {
            return Ok(Self {
                value: Cow::Borrowed(path),
                changed: false,
            });
        };

        if rewritten == path {
            return Ok(Self {
                value: Cow::Borrowed(path),
                changed: false,
            });
        }

        Ok(Self {
            value: Cow::Owned(rewritten),
            changed: true,
        })
    }

    /// Returns the prepared path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Whether preparation rewrote the input.
    ///
    /// A service that wants one canonical spelling per route can redirect
    /// when this is `true` rather than serving the non-canonical spelling.
    #[must_use]
    pub const fn was_changed(&self) -> bool {
        self.changed
    }

    /// Returns the prepared path, still borrowed when nothing was rewritten.
    #[must_use]
    pub fn into_inner(self) -> Cow<'p, str> {
        self.value
    }
}

impl AsRef<str> for PreparedPath<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for PreparedPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Rejects inputs that are not paths, and spellings the policy refuses.
fn validate(path: &str, policy: PathPolicy) -> Result<(), PathError> {
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'?' | b'#' => return Err(PathError::QueryOrFragment),
            b'%' => {
                let hi = bytes.get(index + 1).copied().ok_or(PathError::MalformedEscape)?;
                let lo = bytes.get(index + 2).copied().ok_or(PathError::MalformedEscape)?;
                if !hi.is_ascii_hexdigit() || !lo.is_ascii_hexdigit() {
                    return Err(PathError::MalformedEscape);
                }
                if policy.encoded_separators == EncodedSeparators::Reject && decodes_to_separator(hi, lo) {
                    return Err(PathError::EncodedSeparator);
                }
                index += 2;
            }
            _ => {}
        }
        index += 1;
    }

    let leading_slash = path.starts_with('/');
    let body = if leading_slash { &path[1..] } else { path };
    if body.is_empty() {
        return Ok(());
    }

    let count = body.split('/').count();
    for (position, segment) in body.split('/').enumerate() {
        let last = position + 1 == count;
        if segment.is_empty() {
            if last {
                if policy.trailing_slash == TrailingSlash::Reject {
                    return Err(PathError::TrailingSlash);
                }
            } else if policy.repeated_slashes == RepeatedSlashes::Reject {
                return Err(PathError::RepeatedSlashes);
            }
            continue;
        }
        if is_dot_segment(segment) {
            if policy.dot_segments == DotSegments::Reject {
                return Err(PathError::DotSegment);
            }
            continue;
        }
        if policy.encoded_separators == EncodedSeparators::Reject && decodes_to_dot_segment(segment) {
            return Err(PathError::EncodedSeparator);
        }
    }

    Ok(())
}

/// Whether a validated escape decodes to `/` or `\`.
const fn decodes_to_separator(hi: u8, lo: u8) -> bool {
    let hi = hi.to_ascii_lowercase();
    let lo = lo.to_ascii_lowercase();
    // `%2f` is `/` and `%5c` is `\`, which several intermediaries fold to `/`.
    (hi == b'2' && lo == b'f') || (hi == b'5' && lo == b'c')
}

/// Whether a segment is a literal dot segment.
fn is_dot_segment(segment: &str) -> bool {
    segment == "." || segment == ".."
}

/// Whether a segment that carries an escape would decode to a dot segment.
fn decodes_to_dot_segment(segment: &str) -> bool {
    if !segment.contains('%') {
        return false;
    }
    decode(segment).is_some_and(|decoded| is_dot_segment(&decoded))
}

/// Rewrites `body` under `policy`, or returns [`None`] when the policy asks for
/// no rewriting at all.
fn rewrite(body: &str, leading_slash: bool, policy: PathPolicy) -> Option<String> {
    let collapse = policy.repeated_slashes == RepeatedSlashes::Collapse;
    let remove_dots = policy.dot_segments == DotSegments::Remove;
    let strip_trailing = policy.trailing_slash == TrailingSlash::Remove;
    if !collapse && !remove_dots && !strip_trailing {
        return None;
    }

    let count = body.split('/').count();
    let mut segments: Vec<&str> = Vec::with_capacity(count);
    let mut trailing = false;

    for (position, segment) in body.split('/').enumerate() {
        let last = position + 1 == count;
        if segment.is_empty() {
            if last {
                trailing = true;
            } else if !collapse {
                segments.push(segment);
            }
            continue;
        }
        if remove_dots && is_dot_segment(segment) {
            if segment == ".." {
                // A `..` with nothing to pop would escape above the root, which
                // RFC 3986 discards rather than propagating.
                let _discarded_when_it_would_escape_the_root = segments.pop();
            }
            if last {
                trailing = true;
            }
            continue;
        }
        segments.push(segment);
        if last {
            trailing = false;
        }
    }

    if strip_trailing && !segments.is_empty() {
        trailing = false;
    }

    let mut out = String::with_capacity(body.len() + usize::from(leading_slash));
    if leading_slash {
        out.push('/');
    }
    for (position, segment) in segments.iter().enumerate() {
        if position > 0 {
            out.push('/');
        }
        out.push_str(segment);
    }
    if trailing && !segments.is_empty() {
        out.push('/');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use super::*;

    fn strict(path: &str) -> Result<PreparedPath<'_>, PathError> {
        PreparedPath::new(path, PathPolicy::STRICT)
    }

    #[test]
    fn the_exact_policy_never_rewrites_a_path() {
        for path in [
            "/a/b",
            "/a//b",
            "/a/../b",
            "/a/./b",
            "/a/",
            "/",
            "//",
            "/a/%2e%2e/b",
            "/a/%2F/b",
            "admin/1",
            "",
        ] {
            let prepared = PreparedPath::new(path, PathPolicy::EXACT).expect("the exact policy rejects only non-paths");
            assert_eq!(prepared.as_str(), path, "{path}");
            assert!(!prepared.was_changed(), "{path}");
            assert!(matches!(prepared.into_inner(), Cow::Borrowed(_)), "{path}");
        }
    }

    #[test]
    fn a_query_or_fragment_delimiter_is_rejected_by_every_policy() {
        for policy in [PathPolicy::EXACT, PathPolicy::STRICT] {
            assert_eq!(PreparedPath::new("/a?b=1", policy), Err(PathError::QueryOrFragment));
            assert_eq!(PreparedPath::new("/a#frag", policy), Err(PathError::QueryOrFragment));
        }
    }

    #[test]
    fn a_malformed_escape_is_rejected_by_every_policy() {
        for policy in [PathPolicy::EXACT, PathPolicy::STRICT] {
            for path in ["/a%", "/a%2", "/a%zz", "/a%2z", "/%g0/b"] {
                assert_eq!(PreparedPath::new(path, policy), Err(PathError::MalformedEscape), "{path}");
            }
        }
    }

    #[test]
    fn the_strict_policy_collapses_repeated_separators() {
        assert_eq!(strict("/a//b").expect("collapsed").as_str(), "/a/b");
        assert_eq!(strict("/a///b").expect("collapsed").as_str(), "/a/b");
        assert_eq!(strict("//a").expect("collapsed").as_str(), "/a");
    }

    #[test]
    fn the_strict_policy_resolves_dot_segments_like_the_rfc() {
        for (input, expected) in [
            ("/a/./b", "/a/b"),
            ("/a/../b", "/b"),
            ("/a/b/../c", "/a/c"),
            ("/a/../../b", "/b"),
            ("/../a", "/a"),
            ("/a/b/..", "/a"),
            ("/a/b/.", "/a/b"),
        ] {
            assert_eq!(strict(input).expect("resolved").as_str(), expected, "{input}");
        }
    }

    #[test]
    fn the_strict_policy_drops_a_trailing_separator_but_keeps_the_root() {
        assert_eq!(strict("/a/").expect("stripped").as_str(), "/a");
        assert_eq!(strict("/a/b/").expect("stripped").as_str(), "/a/b");
        let root = strict("/").expect("the root is already canonical");
        assert_eq!(root.as_str(), "/");
        assert!(!root.was_changed());
    }

    #[test]
    fn the_strict_policy_refuses_escapes_that_would_decode_into_structure() {
        for path in [
            "/a/%2F/b",
            "/a/%2f/b",
            "/a/%5C/b",
            "/a/%5c/b",
            "/a/%2e%2e/b",
            "/a/%2E/b",
            "/a/.%2e/b",
        ] {
            assert_eq!(strict(path), Err(PathError::EncodedSeparator), "{path}");
        }
    }

    #[test]
    fn an_encoded_dot_inside_a_longer_segment_is_still_accepted() {
        let prepared = strict("/files/report%2Etxt").expect("an encoded dot is not a dot segment");
        assert_eq!(prepared.as_str(), "/files/report%2Etxt");
        assert!(!prepared.was_changed());
    }

    #[test]
    fn each_rejecting_policy_reports_its_own_error() {
        let reject_slashes = PathPolicy::EXACT.with_repeated_slashes(RepeatedSlashes::Reject);
        assert_eq!(PreparedPath::new("/a//b", reject_slashes), Err(PathError::RepeatedSlashes));

        let reject_dots = PathPolicy::EXACT.with_dot_segments(DotSegments::Reject);
        assert_eq!(PreparedPath::new("/a/../b", reject_dots), Err(PathError::DotSegment));
        assert_eq!(PreparedPath::new("/a/./b", reject_dots), Err(PathError::DotSegment));

        let reject_trailing = PathPolicy::EXACT.with_trailing_slash(TrailingSlash::Reject);
        assert_eq!(PreparedPath::new("/a/", reject_trailing), Err(PathError::TrailingSlash));
        assert!(
            PreparedPath::new("/", reject_trailing).is_ok(),
            "the root is not a trailing separator"
        );
    }

    #[test]
    fn a_path_already_in_canonical_form_is_borrowed_unchanged() {
        for path in ["/a/b", "/", "/a/b/c", "admin/1"] {
            let prepared = strict(path).expect("already canonical");
            assert_eq!(prepared.as_str(), path, "{path}");
            assert!(!prepared.was_changed(), "{path}");
            assert!(matches!(prepared.into_inner(), Cow::Borrowed(_)), "{path}");
        }
    }

    #[test]
    fn preparation_preserves_whether_the_path_had_a_leading_separator() {
        assert_eq!(strict("admin//1/").expect("collapsed").as_str(), "admin/1");
        assert_eq!(strict("/admin//1/").expect("collapsed").as_str(), "/admin/1");
    }

    #[test]
    fn removing_dot_segments_keeps_the_rfc_trailing_separator_when_it_is_preserved() {
        let policy = PathPolicy::EXACT.with_dot_segments(DotSegments::Remove);
        assert_eq!(PreparedPath::new("/a/b/..", policy).expect("resolved").as_str(), "/a/");
        assert_eq!(PreparedPath::new("/a/b/.", policy).expect("resolved").as_str(), "/a/b/");
    }

    #[test]
    fn every_error_renders_a_distinct_message() {
        let messages = [
            PathError::QueryOrFragment.to_string(),
            PathError::MalformedEscape.to_string(),
            PathError::RepeatedSlashes.to_string(),
            PathError::DotSegment.to_string(),
            PathError::TrailingSlash.to_string(),
            PathError::EncodedSeparator.to_string(),
        ];
        for (index, message) in messages.iter().enumerate() {
            assert!(!message.is_empty());
            assert!(!messages[..index].contains(message), "duplicate message `{message}`");
        }
    }

    #[test]
    fn a_prepared_path_renders_and_borrows_as_its_value() {
        let prepared = strict("/a//b").expect("collapsed");
        assert_eq!(prepared.to_string(), "/a/b");
        assert_eq!(prepared.as_ref(), "/a/b");
    }

    #[test]
    fn the_default_policy_is_the_exact_policy() {
        assert_eq!(PathPolicy::default(), PathPolicy::EXACT);
    }
}
