// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

use super::ErrorKind;

/// An error produced while parsing or writing a query string.
///
/// Parsing failures include a byte offset. Production failures have no input
/// offset but retain the affected schema parameter when one is known.
///
/// # Examples
///
/// ```
/// use routerama::query::{ErrorKind, FromQuery};
///
/// #[derive(Debug, FromQuery)]
/// struct Paging {
///     page: usize,
/// }
///
/// let error = Paging::from_query("").expect_err("page is required");
/// assert_eq!(error.parameter(), Some("page"));
/// assert_eq!(error.pair_offset(), Some(0));
/// assert_eq!(error.kind(), ErrorKind::Missing);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    parameter: Option<&'static str>,
    pair_offset: Option<usize>,
    kind: ErrorKind,
}

impl Error {
    pub(crate) const fn parsing(parameter: Option<&'static str>, pair_offset: usize, kind: ErrorKind) -> Self {
        Self {
            parameter,
            pair_offset: Some(pair_offset),
            kind,
        }
    }

    pub(crate) const fn production(parameter: Option<&'static str>, kind: ErrorKind) -> Self {
        Self {
            parameter,
            pair_offset: None,
            kind,
        }
    }

    /// The schema parameter associated with the error, when known.
    #[must_use]
    pub const fn parameter(&self) -> Option<&'static str> {
        self.parameter
    }

    /// The byte offset of the affected query pair, for parsing failures.
    #[must_use]
    pub const fn pair_offset(&self) -> Option<usize> {
        self.pair_offset
    }

    /// The failure category.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        self.kind
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn missing(parameter: &'static str, end_offset: usize) -> Self {
        Self::parsing(Some(parameter), end_offset, ErrorKind::Missing)
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn duplicate(parameter: &'static str, pair_offset: usize) -> Self {
        Self::parsing(Some(parameter), pair_offset, ErrorKind::Duplicate)
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn too_many_values(parameter: &'static str, pair_offset: usize) -> Self {
        Self::parsing(Some(parameter), pair_offset, ErrorKind::TooManyValues)
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn unknown(pair_offset: usize) -> Self {
        Self::parsing(None, pair_offset, ErrorKind::UnknownParameter)
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn ambiguous(pair_offset: usize) -> Self {
        Self::parsing(None, pair_offset, ErrorKind::AmbiguousParameter)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(pair_offset) = self.pair_offset {
            write!(f, "query parsing failed at byte {pair_offset}: {}", self.kind)?;
        } else {
            write!(f, "query production failed: {}", self.kind)?;
        }
        if let Some(parameter) = self.parameter {
            write!(f, " for parameter `{parameter}`")?;
        }
        Ok(())
    }
}

impl core::error::Error for Error {}

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use super::*;

    #[test]
    fn displays_parsing_and_production_context() {
        assert_eq!(
            Error::parsing(None, 7, ErrorKind::InvalidEncoding).to_string(),
            "query parsing failed at byte 7: invalid percent encoding"
        );
        assert_eq!(
            Error::production(None, ErrorKind::Output).to_string(),
            "query production failed: destination writer rejected output"
        );
    }
}
