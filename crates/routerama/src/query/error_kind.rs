// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

/// The category of a query parsing or production failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The encoded query exceeded its configured input length limit.
    QueryTooLong,
    /// The query contained too many pairs.
    TooManyPairs,
    /// Decoded keys and values exceeded their combined length limit.
    DecodedTooLong,
    /// A repeated field contained too many values.
    TooManyValues,
    /// A percent escape was malformed.
    InvalidEncoding,
    /// Percent-decoded bytes were not valid UTF-8.
    InvalidUtf8,
    /// A borrowed field required percent or plus decoding.
    BorrowRequired,
    /// A required parameter was absent.
    Missing,
    /// A scalar parameter occurred more than once.
    Duplicate,
    /// A value could not be parsed into its field type.
    InvalidValue,
    /// An unknown parameter was rejected by the schema.
    UnknownParameter,
    /// More than one flattened field recognized the same parameter.
    AmbiguousParameter,
    /// The encoded output exceeded its configured length limit.
    TooLong,
    /// The destination writer rejected output.
    Output,
    /// A field's [`core::fmt::Display`] implementation failed.
    Format,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::QueryTooLong => "input exceeds the configured length limit",
            Self::TooManyPairs => "query contains too many pairs",
            Self::DecodedTooLong => "decoded data exceeds the configured length limit",
            Self::TooManyValues => "parameter has too many values",
            Self::InvalidEncoding => "invalid percent encoding",
            Self::InvalidUtf8 => "decoded bytes are not valid UTF-8",
            Self::BorrowRequired => "value requires decoding but the destination field must borrow",
            Self::Missing => "required parameter is missing",
            Self::Duplicate => "parameter occurs more than once",
            Self::InvalidValue => "parameter value is invalid",
            Self::UnknownParameter => "parameter is not recognized by the schema",
            Self::AmbiguousParameter => "parameter is claimed by multiple flattened fields",
            Self::TooLong => "encoded output exceeds the configured length limit",
            Self::Output => "destination writer rejected output",
            Self::Format => "field formatting failed",
        })
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString as _;

    use super::*;

    #[test]
    fn every_kind_has_a_human_readable_message() {
        for (kind, expected) in [
            (ErrorKind::QueryTooLong, "input exceeds the configured length limit"),
            (ErrorKind::TooManyPairs, "query contains too many pairs"),
            (ErrorKind::DecodedTooLong, "decoded data exceeds the configured length limit"),
            (ErrorKind::TooManyValues, "parameter has too many values"),
            (ErrorKind::InvalidEncoding, "invalid percent encoding"),
            (ErrorKind::InvalidUtf8, "decoded bytes are not valid UTF-8"),
            (
                ErrorKind::BorrowRequired,
                "value requires decoding but the destination field must borrow",
            ),
            (ErrorKind::Missing, "required parameter is missing"),
            (ErrorKind::Duplicate, "parameter occurs more than once"),
            (ErrorKind::InvalidValue, "parameter value is invalid"),
            (ErrorKind::UnknownParameter, "parameter is not recognized by the schema"),
            (ErrorKind::AmbiguousParameter, "parameter is claimed by multiple flattened fields"),
            (ErrorKind::TooLong, "encoded output exceeds the configured length limit"),
            (ErrorKind::Output, "destination writer rejected output"),
            (ErrorKind::Format, "field formatting failed"),
        ] {
            assert_eq!(kind.to_string(), expected);
        }
    }
}
