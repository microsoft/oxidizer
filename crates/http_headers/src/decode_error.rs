// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Structured decode errors shared by every header parser.

use std::error::Error;
use std::{fmt, mem};

use crate::FieldName;

/// The reason a header could not be decoded.
///
/// # Examples
///
/// ```rust
/// use http_headers::DecodeErrorKind;
///
/// assert_eq!(DecodeErrorKind::MissingValue.to_string(), "missing value");
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum DecodeErrorKind {
    /// No field value was present.
    MissingValue,

    /// A singleton header contained more than one field value.
    UnexpectedMultipleValues,

    /// The field value did not match the header grammar.
    InvalidSyntax,

    /// A component that requires UTF-8 contained other bytes.
    InvalidUtf8,

    /// A token contained a byte forbidden by the HTTP token grammar.
    InvalidToken,

    /// A numeric component was invalid or out of range.
    InvalidNumber,

    /// A quoted value was not terminated.
    UnterminatedQuote,

    /// A cached decoding result had an unexpected type.
    CacheTypeMismatch,
}

/// An error produced while decoding a header.
///
/// Raw field values are deliberately omitted because headers can contain
/// credentials and other secrets.
///
/// # Examples
///
/// ```rust
/// use http_headers::{DecodeError, DecodeErrorKind, FieldName};
///
/// let error = DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax);
/// assert_eq!(error.header(), &FieldName::ContentType);
/// assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DecodeError {
    header: &'static FieldName,
    value_index: u32,
    kind: DecodeErrorKind,
}

#[cfg(target_pointer_width = "64")]
const _: [(); 16] = [(); mem::size_of::<DecodeError>()];

impl DecodeError {
    const NO_VALUE_INDEX: u32 = u32::MAX;

    /// Creates an error for `header`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::{DecodeError, DecodeErrorKind, FieldName};
    ///
    /// let error = DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax);
    /// assert_eq!(error.value_index(), None);
    /// ```
    #[must_use]
    pub const fn new(header: &'static FieldName, kind: DecodeErrorKind) -> Self {
        Self {
            header,
            value_index: Self::NO_VALUE_INDEX,
            kind,
        }
    }

    /// Attaches the zero-based field-value index at which decoding failed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::{DecodeError, DecodeErrorKind, FieldName};
    ///
    /// let error =
    ///     DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax).at_value(2);
    /// assert_eq!(error.value_index(), Some(2));
    /// ```
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the preceding bound check proves the index fits in the compact representation"
    )]
    pub const fn at_value(mut self, value_index: usize) -> Self {
        self.value_index = if value_index >= Self::NO_VALUE_INDEX as usize {
            Self::NO_VALUE_INDEX - 1
        } else {
            value_index as u32
        };
        self
    }

    /// Returns the affected header name.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::{DecodeError, DecodeErrorKind, FieldName};
    ///
    /// let error = DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax);
    /// assert_eq!(error.header(), &FieldName::ContentType);
    /// ```
    #[must_use]
    pub const fn header(&self) -> &'static FieldName {
        self.header
    }

    /// Returns the zero-based field-value index, when known.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::{DecodeError, DecodeErrorKind, FieldName};
    ///
    /// let error = DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax);
    /// assert_eq!(error.value_index(), None);
    /// assert_eq!(error.at_value(1).value_index(), Some(1));
    /// ```
    #[must_use]
    pub const fn value_index(&self) -> Option<usize> {
        if self.value_index == Self::NO_VALUE_INDEX {
            None
        } else {
            Some(self.value_index as usize)
        }
    }

    /// Returns the structured failure reason.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::{DecodeError, DecodeErrorKind, FieldName};
    ///
    /// let error = DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax);
    /// assert_eq!(error.kind(), DecodeErrorKind::InvalidSyntax);
    /// ```
    #[must_use]
    pub const fn kind(&self) -> DecodeErrorKind {
        self.kind
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {} header: {}", self.header.as_str(), KindDisplay(self.kind))?;
        if let Some(index) = self.value_index() {
            write!(f, " at value {index}")?;
        }
        Ok(())
    }
}

impl Error for DecodeError {}

impl fmt::Display for DecodeErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MissingValue => "missing value",
            Self::UnexpectedMultipleValues => "unexpected multiple values",
            Self::InvalidSyntax => "invalid syntax",
            Self::InvalidUtf8 => "invalid UTF-8",
            Self::InvalidToken => "invalid token",
            Self::InvalidNumber => "invalid number",
            Self::UnterminatedQuote => "unterminated quoted string",
            Self::CacheTypeMismatch => "internal cache type mismatch",
        })
    }
}

struct KindDisplay(DecodeErrorKind);

impl fmt::Display for KindDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error;
    use std::fmt::{self, Write};

    use super::{DecodeError, DecodeErrorKind};
    use crate::FieldName;

    struct FailAfter {
        writes_left: usize,
    }

    impl Write for FailAfter {
        fn write_str(&mut self, _value: &str) -> fmt::Result {
            if self.writes_left == 0 {
                Err(fmt::Error)
            } else {
                self.writes_left -= 1;
                Ok(())
            }
        }
    }

    #[test]
    fn error_accessors_display_and_index_saturation_are_structured() {
        let unindexed = DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax);
        assert_eq!(unindexed.header(), &FieldName::ContentType);
        assert_eq!(unindexed.value_index(), None);
        assert_eq!(unindexed.kind(), DecodeErrorKind::InvalidSyntax);
        assert_eq!(unindexed.to_string(), "invalid content-type header: invalid syntax");

        let indexed = unindexed.at_value(7);
        assert_eq!(indexed.value_index(), Some(7));
        assert_eq!(indexed.to_string(), "invalid content-type header: invalid syntax at value 7");

        let saturated = unindexed.at_value(usize::MAX);
        assert_eq!(saturated.value_index(), Some(u32::MAX as usize - 1));
        let error: &dyn Error = &saturated;
        assert!(error.source().is_none());
    }

    #[test]
    fn every_decode_kind_has_stable_nonsensitive_text() {
        let cases = [
            (DecodeErrorKind::MissingValue, "missing value"),
            (DecodeErrorKind::UnexpectedMultipleValues, "unexpected multiple values"),
            (DecodeErrorKind::InvalidSyntax, "invalid syntax"),
            (DecodeErrorKind::InvalidUtf8, "invalid UTF-8"),
            (DecodeErrorKind::InvalidToken, "invalid token"),
            (DecodeErrorKind::InvalidNumber, "invalid number"),
            (DecodeErrorKind::UnterminatedQuote, "unterminated quoted string"),
            (DecodeErrorKind::CacheTypeMismatch, "internal cache type mismatch"),
        ];
        for (kind, expected) in cases {
            assert_eq!(kind.to_string(), expected);
        }
    }

    #[test]
    fn display_propagates_formatter_failures() {
        let error = DecodeError::new(&FieldName::ContentType, DecodeErrorKind::InvalidSyntax).at_value(3);
        let mut immediate = FailAfter { writes_left: 0 };
        assert!(immediate.write_fmt(format_args!("{error}")).is_err());

        let mut after_message = FailAfter { writes_left: 4 };
        assert!(after_message.write_fmt(format_args!("{error}")).is_err());
    }
}
