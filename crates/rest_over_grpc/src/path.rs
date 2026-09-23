// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Request query-string parsing helpers.
//!
//! Path/segment/verb scanning lives in the [`routerama`] runtime
//! (`scan_segments` / `split_verb`); this module adds only the
//! REST-specific query-string helpers layered on top.

use core::ops::Index;
use core::slice::{self, SliceIndex};

use crate::transcode::TranscodeError;

/// Maximum number of query pairs accepted by the generated request path.
pub(crate) const MAX_QUERY_PAIRS: usize = 128;
/// Maximum aggregate raw query-name bytes accepted by the generated request path.
pub(crate) const MAX_QUERY_KEY_BYTES: usize = 4096;
/// Maximum raw query length, including empty separators and value bytes.
pub(crate) const MAX_QUERY_BYTES: usize = 16 * 1024;

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
    /// Parses a raw query with the request overlay's fixed work budget.
    ///
    /// Unlike [`parse_query`], rejects excess pairs or aggregate key bytes
    /// incrementally, before collecting the remainder of a large query.
    /// The raw query is limited to 16 KiB as well, so even empty separators
    /// and long values cannot bypass the work budget.
    ///
    /// # Errors
    ///
    /// Returns an invalid-argument transcoding error when the query exceeds
    /// 128 pairs, 4096 aggregate raw name bytes, or 16 KiB in total.
    pub fn parse_limited(query: &'a str) -> Result<Self, TranscodeError> {
        if query.len() > MAX_QUERY_BYTES {
            return Err(TranscodeError::structure("query exceeds the request query size limit"));
        }
        let mut pairs = Self::default();
        let mut key_bytes = 0usize;
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            key_bytes = key_bytes.saturating_add(key.len());
            if pairs.len() == MAX_QUERY_PAIRS || key_bytes > MAX_QUERY_KEY_BYTES {
                return Err(TranscodeError::structure(
                    "query exceeds the parameter count or field-name size limit",
                ));
            }
            pairs.pairs.push((key, value));
        }
        Ok(pairs)
    }

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
    fn limited_parser_rejects_oversize_input_before_collecting_it() {
        let at_limit = "a=1&".repeat(MAX_QUERY_PAIRS);
        assert_eq!(QueryPairs::parse_limited(&at_limit).unwrap().len(), MAX_QUERY_PAIRS);
        let over_limit = format!("{at_limit}a=1");
        assert_eq!(
            QueryPairs::parse_limited(&over_limit).unwrap_err().code(),
            crate::handling::Code::InvalidArgument
        );
        let name = "x".repeat(MAX_QUERY_KEY_BYTES);
        assert_eq!(QueryPairs::parse_limited(&name).unwrap().len(), 1);
        assert_eq!(
            QueryPairs::parse_limited(&format!("{name}x")).unwrap_err().code(),
            crate::handling::Code::InvalidArgument
        );
        assert_eq!(
            QueryPairs::parse_limited(&"&".repeat(MAX_QUERY_BYTES + 1)).unwrap_err().code(),
            crate::handling::Code::InvalidArgument
        );
        let exact_raw_limit = format!("a={}", "x".repeat(MAX_QUERY_BYTES - 2));
        assert_eq!(
            QueryPairs::parse_limited(&exact_raw_limit).unwrap()[0],
            ("a", &exact_raw_limit[2..])
        );
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
