// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Resource limits applied while parsing or producing a query string.
///
/// # Examples
///
/// ```
/// use routerama::query::{FromQuery, QueryLimits};
///
/// #[derive(routerama::query::FromQuery)]
/// struct Search {
///     q: String,
/// }
///
/// let limits = QueryLimits {
///     max_query_length: 3,
///     ..QueryLimits::DEFAULT
/// };
/// assert!(Search::from_query_with("q=rust", limits).is_err());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(clippy::struct_field_names, reason = "the max_ prefix makes each public limit unambiguous")]
pub struct QueryLimits {
    /// Maximum encoded input length.
    pub max_query_length: usize,
    /// Maximum number of non-empty query pairs.
    pub max_pairs: usize,
    /// Maximum combined decoded key and value length.
    pub max_decoded_length: usize,
    /// Maximum values accepted by any repeated field.
    pub max_repeated_values: usize,
    /// Maximum encoded output length.
    pub max_encoded_length: usize,
}

impl QueryLimits {
    /// Limits suitable for HTTP request query strings.
    pub const DEFAULT: Self = Self {
        max_query_length: 16 * 1024,
        max_pairs: 256,
        max_decoded_length: 64 * 1024,
        max_repeated_values: 256,
        max_encoded_length: 64 * 1024,
    };

    /// Disables all codec resource limits.
    pub const UNLIMITED: Self = Self {
        max_query_length: usize::MAX,
        max_pairs: usize::MAX,
        max_decoded_length: usize::MAX,
        max_repeated_values: usize::MAX,
        max_encoded_length: usize::MAX,
    };
}

impl Default for QueryLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
