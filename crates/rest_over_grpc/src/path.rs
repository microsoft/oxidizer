// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Request query-string parsing helpers.
//!
//! Path/segment/verb scanning lives in the [`routerama`] runtime
//! (`scan_segments` / `split_verb`); this module adds only the
//! REST-specific query-string helpers layered on top.

use core::ops::Deref;

/// Splits a request path-and-query string into the path and the raw query
/// string (the part after the first `?`), if any.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::codegen_helpers::split_query;
///
/// assert_eq!(
///     split_query("/shelves/7?theme=history"),
///     ("/shelves/7", Some("theme=history"))
/// );
/// assert_eq!(split_query("/shelves/7"), ("/shelves/7", None));
/// ```
#[must_use]
pub fn split_query(target: &str) -> (&str, Option<&str>) {
    match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    }
}

/// The `(key, value)` pairs of a parsed query string.
///
/// Up to eight pairs are stored inline. The value dereferences to a slice.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::codegen_helpers::parse_query;
///
/// let pairs = parse_query("theme=history&limit=10");
/// assert_eq!(pairs.len(), 2);
/// assert_eq!(pairs[0], ("theme", "history"));
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct QueryPairs<'a> {
    pairs: smallvec::SmallVec<[(&'a str, &'a str); 8]>,
}

impl<'a> Deref for QueryPairs<'a> {
    type Target = [(&'a str, &'a str)];

    fn deref(&self) -> &Self::Target {
        &self.pairs
    }
}

/// Parses a raw query string into its [`QueryPairs`].
///
/// Keys without `=` are treated as having an empty value. No percent-decoding
/// is performed; callers needing it should decode beforehand.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::codegen_helpers::parse_query;
///
/// let pairs = parse_query("theme=history&show_deleted&limit=10");
/// assert_eq!(
///     &*pairs,
///     [("theme", "history"), ("show_deleted", ""), ("limit", "10")]
/// );
/// ```
#[must_use]
pub fn parse_query(query: &str) -> QueryPairs<'_> {
    QueryPairs {
        pairs: query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| match pair.split_once('=') {
                Some((key, value)) => (key, value),
                None => (pair, ""),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_query() {
        assert_eq!(split_query("/v1/x?a=1&b=2"), ("/v1/x", Some("a=1&b=2")));
        assert_eq!(split_query("/v1/x"), ("/v1/x", None));
    }

    #[test]
    fn parses_query_pairs() {
        assert_eq!(&*parse_query("a=1&b=2&flag"), [("a", "1"), ("b", "2"), ("flag", "")]);
        assert!(parse_query("").is_empty());
    }
}
