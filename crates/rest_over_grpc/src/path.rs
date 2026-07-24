// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Request query-string parsing helpers.
//!
//! Path/segment/verb scanning lives in the [`routerama`] runtime
//! (`scan_segments` / `split_verb`); this module adds only the
//! REST-specific query-string helpers layered on top.

use core::ops::Index;
use core::slice::{self, SliceIndex};

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
/// Up to eight pairs are stored inline. Use [`as_slice`](Self::as_slice) or
/// iterate by value or reference.
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
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
pub struct QueryPairs<'a> {
    pairs: smallvec::SmallVec<[(&'a str, &'a str); 8]>,
}

impl<'a> QueryPairs<'a> {
    /// Returns the parsed pairs as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[(&'a str, &'a str)] {
        self.pairs.as_slice()
    }

    /// Returns an iterator over the parsed pairs.
    pub fn iter(&self) -> slice::Iter<'_, (&'a str, &'a str)> {
        self.pairs.iter()
    }

    /// Returns the number of parsed pairs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    /// Returns whether the query contained no pairs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

impl<'a> AsRef<[(&'a str, &'a str)]> for QueryPairs<'a> {
    fn as_ref(&self) -> &[(&'a str, &'a str)] {
        self.as_slice()
    }
}

impl<'a, I> Index<I> for QueryPairs<'a>
where
    I: SliceIndex<[(&'a str, &'a str)]>,
{
    type Output = I::Output;

    fn index(&self, index: I) -> &Self::Output {
        &self.as_slice()[index]
    }
}

impl<'a> Extend<(&'a str, &'a str)> for QueryPairs<'a> {
    fn extend<T: IntoIterator<Item = (&'a str, &'a str)>>(&mut self, iter: T) {
        self.pairs.extend(iter);
    }
}

impl<'a> FromIterator<(&'a str, &'a str)> for QueryPairs<'a> {
    fn from_iter<T: IntoIterator<Item = (&'a str, &'a str)>>(iter: T) -> Self {
        Self {
            pairs: iter.into_iter().collect(),
        }
    }
}

impl<'a> IntoIterator for QueryPairs<'a> {
    type Item = (&'a str, &'a str);
    type IntoIter = smallvec::IntoIter<[(&'a str, &'a str); 8]>;

    fn into_iter(self) -> Self::IntoIter {
        self.pairs.into_iter()
    }
}

impl<'a, 'query> IntoIterator for &'a QueryPairs<'query> {
    type Item = &'a (&'query str, &'query str);
    type IntoIter = slice::Iter<'a, (&'query str, &'query str)>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
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
///     pairs.as_slice(),
///     [("theme", "history"), ("show_deleted", ""), ("limit", "10")]
/// );
/// ```
#[must_use]
pub fn parse_query(query: &str) -> QueryPairs<'_> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        })
        .collect()
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
        assert_eq!(parse_query("a=1&b=2&flag").as_slice(), [("a", "1"), ("b", "2"), ("flag", "")]);
        assert!(parse_query("").is_empty());
        assert!(!parse_query("a=1").is_empty());
    }

    #[test]
    fn supports_standard_collection_traits() {
        let mut pairs: QueryPairs<'_> = [("a", "1"), ("b", "2")].into_iter().collect();
        pairs.extend([("c", "3")]);

        assert_eq!(pairs.as_ref(), [("a", "1"), ("b", "2"), ("c", "3")]);
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs.iter().copied().collect::<Vec<_>>(), [("a", "1"), ("b", "2"), ("c", "3")]);
        assert_eq!(pairs[1], ("b", "2"));
        assert_eq!(
            (&pairs).into_iter().copied().collect::<Vec<_>>(),
            [("a", "1"), ("b", "2"), ("c", "3")]
        );
        assert_eq!(pairs.into_iter().collect::<Vec<_>>(), [("a", "1"), ("b", "2"), ("c", "3")]);
    }
}
