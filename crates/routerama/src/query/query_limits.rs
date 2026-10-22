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
/// let limits = QueryLimits::DEFAULT.with_max_query_length(3);
/// assert!(Search::from_query_with("q=rust", limits).is_err());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
    ///
    /// These limits are round-trip safe: any query the codec produces under
    /// `DEFAULT` parses back under `DEFAULT`. The production limits are
    /// therefore no larger than the matching parsing limits, so a value that
    /// cannot be parsed back is refused while it is being produced rather than
    /// yielding a query string that no parser configured the same way accepts.
    ///
    /// Neither [`UNLIMITED`](Self::UNLIMITED) nor hand-built limits carry that
    /// guarantee. Limits that allow more output than input, or more produced
    /// pairs than parsed pairs, can produce query strings that the same limits
    /// reject on the way back in.
    pub const DEFAULT: Self = Self {
        max_query_length: 16 * 1024,
        max_pairs: 256,
        max_decoded_length: 64 * 1024,
        max_repeated_values: 256,
        max_encoded_length: 16 * 1024,
    };

    /// Disables all codec resource limits.
    pub const UNLIMITED: Self = Self {
        max_query_length: usize::MAX,
        max_pairs: usize::MAX,
        max_decoded_length: usize::MAX,
        max_repeated_values: usize::MAX,
        max_encoded_length: usize::MAX,
    };

    /// Returns these limits with [`max_query_length`](Self::max_query_length) replaced.
    #[must_use]
    pub const fn with_max_query_length(mut self, value: usize) -> Self {
        self.max_query_length = value;
        self
    }

    /// Returns these limits with [`max_pairs`](Self::max_pairs) replaced.
    #[must_use]
    pub const fn with_max_pairs(mut self, value: usize) -> Self {
        self.max_pairs = value;
        self
    }

    /// Returns these limits with [`max_decoded_length`](Self::max_decoded_length) replaced.
    #[must_use]
    pub const fn with_max_decoded_length(mut self, value: usize) -> Self {
        self.max_decoded_length = value;
        self
    }

    /// Returns these limits with [`max_repeated_values`](Self::max_repeated_values) replaced.
    #[must_use]
    pub const fn with_max_repeated_values(mut self, value: usize) -> Self {
        self.max_repeated_values = value;
        self
    }

    /// Returns these limits with [`max_encoded_length`](Self::max_encoded_length) replaced.
    #[must_use]
    pub const fn with_max_encoded_length(mut self, value: usize) -> Self {
        self.max_encoded_length = value;
        self
    }
}

impl Default for QueryLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
