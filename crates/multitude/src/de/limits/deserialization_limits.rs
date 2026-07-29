// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Resource limits applied while deserializing an arena graph.
///
/// Start with [`Self::unlimited`] and set only the bounds needed by the input:
///
/// ```
/// use multitude::de::DeserializationLimits;
///
/// let limits = DeserializationLimits::unlimited()
///     .with_max_depth(32)
///     .with_max_sequence_len(1_000)
///     .with_max_string_len(64 * 1024);
/// assert_eq!(limits.max_depth, 32);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[expect(
    clippy::struct_field_names,
    reason = "the max prefix makes each public policy field unambiguous at call sites"
)]
pub struct DeserializationLimits {
    /// Maximum nesting depth below the root value.
    pub max_depth: usize,
    /// Maximum number of elements in any sequence.
    pub max_sequence_len: usize,
    /// Maximum number of entries in any map.
    pub max_map_len: usize,
    /// Maximum UTF-8 byte length of a string.
    pub max_string_len: usize,
    /// Maximum length of a byte string.
    pub max_bytes_len: usize,
}

impl DeserializationLimits {
    /// Allow values of any representable size.
    ///
    /// ```
    /// let limits = multitude::de::DeserializationLimits::unlimited();
    /// assert_eq!(limits.max_depth, usize::MAX);
    /// ```
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            max_depth: usize::MAX,
            max_sequence_len: usize::MAX,
            max_map_len: usize::MAX,
            max_string_len: usize::MAX,
            max_bytes_len: usize::MAX,
        }
    }

    /// Set the maximum nesting depth below the root value.
    ///
    /// ```
    /// let limits = multitude::de::DeserializationLimits::unlimited().with_max_depth(16);
    /// assert_eq!(limits.max_depth, 16);
    /// ```
    #[must_use]
    pub const fn with_max_depth(mut self, max: usize) -> Self {
        self.max_depth = max;
        self
    }

    /// Set the maximum number of elements in any sequence.
    ///
    /// ```
    /// let limits = multitude::de::DeserializationLimits::unlimited().with_max_sequence_len(100);
    /// assert_eq!(limits.max_sequence_len, 100);
    /// ```
    #[must_use]
    pub const fn with_max_sequence_len(mut self, max: usize) -> Self {
        self.max_sequence_len = max;
        self
    }

    /// Set the maximum number of entries in any map.
    ///
    /// ```
    /// let limits = multitude::de::DeserializationLimits::unlimited().with_max_map_len(100);
    /// assert_eq!(limits.max_map_len, 100);
    /// ```
    #[must_use]
    pub const fn with_max_map_len(mut self, max: usize) -> Self {
        self.max_map_len = max;
        self
    }

    /// Set the maximum UTF-8 byte length of a string.
    ///
    /// ```
    /// let limits = multitude::de::DeserializationLimits::unlimited().with_max_string_len(4096);
    /// assert_eq!(limits.max_string_len, 4096);
    /// ```
    #[must_use]
    pub const fn with_max_string_len(mut self, max: usize) -> Self {
        self.max_string_len = max;
        self
    }

    /// Set the maximum length of a byte string.
    ///
    /// ```
    /// let limits = multitude::de::DeserializationLimits::unlimited().with_max_bytes_len(4096);
    /// assert_eq!(limits.max_bytes_len, 4096);
    /// ```
    #[must_use]
    pub const fn with_max_bytes_len(mut self, max: usize) -> Self {
        self.max_bytes_len = max;
        self
    }
}

impl Default for DeserializationLimits {
    fn default() -> Self {
        Self::unlimited()
    }
}
