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

    /// A source or typed-construction byte, field-line, or list-item budget was exceeded.
    ///
    /// This is an admission limit, not a statement that the field bytes or
    /// grammar are invalid.
    SourceLimitExceeded,
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
            Self::SourceLimitExceeded => "source limit exceeded",
        })
    }
}

struct KindDisplay(DecodeErrorKind);

impl fmt::Display for KindDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
