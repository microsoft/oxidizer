// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Borrowed projections of retained URI-reference metadata.

use std::fmt;
use std::hash::{Hash, Hasher};

use super::{Metadata, UriAuthority};

/// A validated URI-reference with constant-time component projections.
///
/// Components retain percent escapes: `%2F` stays part of its component,
/// rather than becoming a path separator. Query and fragment accessors
/// distinguish absence from a present-but-empty component. An empty path is
/// valid. No resolving, dot-segment removal, case folding, or percent decoding
/// is performed.
///
/// In relaxed mode this view describes the spelling with backslashes replaced
/// by slashes. The containing [`super::LocationOwned`] or [`super::LocationView`]
/// still exposes and forwards the original wire value.
///
/// Equality and hashing compare this semantic spelling exactly. This is not
/// URI equivalence under normalization or scheme-specific rules, and is not a
/// safe-redirect authorization policy.
///
/// # Examples
///
/// ```
/// use http_headers::headers::LocationOwned;
///
/// let location = LocationOwned::try_from("https://example.com/a%2Fb?#")?;
/// let uri = location.uri_reference();
/// assert_eq!(uri.scheme(), Some("https"));
/// assert_eq!(uri.authority().unwrap().host(), "example.com");
/// assert_eq!(uri.path(), "/a%2Fb");
/// assert_eq!(uri.query(), Some(""));
/// assert_eq!(uri.fragment(), Some(""));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
#[derive(Clone, Copy)]
pub struct UriReference<'a> {
    text: &'a str,
    scheme: Option<&'a str>,
    authority: Option<UriAuthority<'a>>,
    path: &'a str,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

impl<'a> UriReference<'a> {
    pub(super) fn from_metadata(text: &'a str, metadata: &Metadata) -> Self {
        let authority = metadata.authority_host.as_ref().map(|host| {
            let start = metadata.scheme_end.map_or(2, |end| end.get() + 3);
            UriAuthority {
                userinfo: (host.start != start).then(|| &text[start..host.start - 1]),
                host: &text[host.clone()],
                port: (host.end != metadata.path.start).then(|| &text[host.end + 1..metadata.path.start]),
            }
        });
        Self {
            text,
            scheme: metadata.scheme_end.map(|end| &text[..end.get()]),
            authority,
            path: &text[metadata.path.clone()],
            query: metadata.query_end.map(|end| &text[metadata.path.end + 1..end.get()]),
            fragment: metadata.fragment_start.map(|start| &text[start.get()..]),
        }
    }

    /// Returns the complete semantic spelling, normalized in relaxed mode.
    #[inline]
    #[must_use]
    pub const fn as_str(self) -> &'a str {
        self.text
    }

    /// Returns the scheme without its colon, preserving case.
    #[inline]
    #[must_use]
    pub const fn scheme(self) -> Option<&'a str> {
        self.scheme
    }

    /// Returns structured authority components, if `//` introduces an authority.
    ///
    /// `Some` may contain an empty host; it is distinct from no authority.
    #[inline]
    #[must_use]
    pub const fn authority(self) -> Option<UriAuthority<'a>> {
        self.authority
    }

    /// Returns the encoded path, which may be empty.
    #[inline]
    #[must_use]
    pub const fn path(self) -> &'a str {
        self.path
    }

    /// Returns the encoded query without `?`, distinguishing absent from empty.
    #[inline]
    #[must_use]
    pub const fn query(self) -> Option<&'a str> {
        self.query
    }

    /// Returns the encoded fragment without `#`, distinguishing absent from empty.
    #[inline]
    #[must_use]
    pub const fn fragment(self) -> Option<&'a str> {
        self.fragment
    }
}

impl PartialEq for UriReference<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for UriReference<'_> {}

impl Hash for UriReference<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text.hash(state);
    }
}

impl fmt::Debug for UriReference<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UriReference").field("redacted", &true).finish_non_exhaustive()
    }
}
