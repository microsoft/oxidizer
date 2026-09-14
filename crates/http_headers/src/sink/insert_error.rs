// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The error reported when a field cannot be encoded or stored.

use std::error::Error;
use std::fmt;

/// The category of an encoding or storage failure.
///
/// Categories describe the failure, not whether retrying will succeed.
///
/// # Examples
///
/// ```
/// use http_headers::sink::{InsertError, InsertErrorKind};
///
/// let error = InsertError::new(InsertErrorKind::CapacityExceeded);
/// assert_eq!(error.kind(), InsertErrorKind::CapacityExceeded);
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum InsertErrorKind {
    /// A value does not satisfy the field's grammar or HTTP field-byte rules.
    InvalidValue,
    /// An encoder wrote a different number of bytes than it announced.
    InvalidEncoding,
    /// A buffer reservation failed.
    ///
    /// The built-in sinks check supported size limits before reserving.
    /// This category alone does not mean retrying will succeed.
    AllocationFailed,
    /// A value or container would exceed a supported size limit.
    CapacityExceeded,
}

impl fmt::Display for InsertErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidValue => "invalid field value",
            Self::InvalidEncoding => "encoded field length does not match the announced length",
            Self::AllocationFailed => "field buffer capacity could not be reserved",
            Self::CapacityExceeded => "field value or container capacity exceeded",
        })
    }
}

/// An error produced when a field cannot be stored.
///
/// The error retains a compact category, not field contents or underlying
/// errors. This keeps it [`Copy`] and avoids retaining sensitive values.
///
/// # Examples
///
/// ```rust
/// # #[cfg(all(feature = "http", feature = "headers-user-agent"))]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{UserAgent, UserAgentOwned};
/// use http_headers::sink::InsertError;
///
/// let mut map = HeaderMap::new();
/// let stored: Result<(), InsertError> =
///     UserAgent::insert(&mut map, UserAgentOwned::try_from_static("client/1")?);
/// assert!(stored.is_ok());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(all(feature = "http", feature = "headers-user-agent")))]
/// # fn main() {}
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InsertError {
    kind: InsertErrorKind,
}

impl InsertError {
    /// Creates an error with the supplied failure category.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::sink::{InsertError, InsertErrorKind};
    ///
    /// let error = InsertError::new(InsertErrorKind::InvalidValue);
    /// assert_eq!(error.kind(), InsertErrorKind::InvalidValue);
    /// ```
    #[must_use]
    pub const fn new(kind: InsertErrorKind) -> Self {
        Self { kind }
    }

    /// Returns the encoding or storage failure category.
    ///
    /// # Examples
    ///
    /// ```
    /// use http_headers::sink::{InsertError, InsertErrorKind};
    ///
    /// let error = InsertError::new(InsertErrorKind::InvalidEncoding);
    /// assert_eq!(error.kind(), InsertErrorKind::InvalidEncoding);
    /// ```
    #[must_use]
    pub const fn kind(self) -> InsertErrorKind {
        self.kind
    }
}

impl fmt::Display for InsertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(f)
    }
}

impl Error for InsertError {}
