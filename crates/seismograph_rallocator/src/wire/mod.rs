// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::renamed_function_params,
    reason = "Implementation parameter names are clearer than generic trait names"
)]

//! Allocation-free snapshot wire primitives.

#[cfg(test)]
mod container_tests;
pub(crate) mod format;
pub(crate) mod io;

/// An error reported while reading or writing the wire format.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Error {
    kind: ErrorKind,
}

/// Stable category of a wire-format error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum ErrorKind {
    /// The input or output ended before the operation completed.
    UnexpectedEnd,
    /// The container magic bytes are invalid.
    InvalidMagic,
    /// The container uses an unsupported wire-format version.
    UnsupportedWireFormat(u16),
    /// A length cannot be represented by the wire format.
    LengthOverflow,
    /// The output contains unused trailing bytes.
    TrailingBytes,
    /// A reserved field contains a nonzero value.
    InvalidReserved,
    /// A section payload differs from its declared length.
    SectionLengthMismatch,
}

impl Error {
    pub(crate) const UNEXPECTED_END: Self = Self::new(ErrorKind::UnexpectedEnd);
    pub(crate) const INVALID_MAGIC: Self = Self::new(ErrorKind::InvalidMagic);
    pub(crate) const LENGTH_OVERFLOW: Self = Self::new(ErrorKind::LengthOverflow);
    pub(crate) const TRAILING_BYTES: Self = Self::new(ErrorKind::TrailingBytes);
    pub(crate) const INVALID_RESERVED: Self = Self::new(ErrorKind::InvalidReserved);
    pub(crate) const SECTION_LENGTH_MISMATCH: Self = Self::new(ErrorKind::SectionLengthMismatch);

    const fn new(kind: ErrorKind) -> Self {
        Self { kind }
    }

    /// Returns the stable category of this error.
    #[must_use]
    #[cfg(test)]
    pub(crate) const fn kind(self) -> ErrorKind {
        self.kind
    }

    pub(crate) const fn unsupported_wire_format(version: u16) -> Self {
        Self::new(ErrorKind::UnsupportedWireFormat(version))
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.kind {
            ErrorKind::UnexpectedEnd => formatter.write_str("the input or output ended unexpectedly"),
            ErrorKind::InvalidMagic => formatter.write_str("the container magic bytes are invalid"),
            ErrorKind::UnsupportedWireFormat(version) => write!(formatter, "wire-format version {version} is unsupported"),
            ErrorKind::LengthOverflow => formatter.write_str("a length cannot be represented"),
            ErrorKind::TrailingBytes => formatter.write_str("the output contains unused trailing bytes"),
            ErrorKind::InvalidReserved => formatter.write_str("a reserved field contains a nonzero value"),
            ErrorKind::SectionLengthMismatch => formatter.write_str("a section payload differs from its declared length"),
        }
    }
}

impl core::fmt::Debug for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "Error({self})")
    }
}

impl core::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_kind_exposes_stable_categories() {
        let cases = [
            (Error::UNEXPECTED_END, ErrorKind::UnexpectedEnd),
            (Error::INVALID_MAGIC, ErrorKind::InvalidMagic),
            (Error::unsupported_wire_format(7), ErrorKind::UnsupportedWireFormat(7)),
            (Error::LENGTH_OVERFLOW, ErrorKind::LengthOverflow),
            (Error::TRAILING_BYTES, ErrorKind::TrailingBytes),
            (Error::INVALID_RESERVED, ErrorKind::InvalidReserved),
            (Error::SECTION_LENGTH_MISMATCH, ErrorKind::SectionLengthMismatch),
        ];

        for (error, kind) in cases {
            assert_eq!(error.kind(), kind);
        }
    }
}
