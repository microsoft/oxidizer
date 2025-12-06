// Copyright (c) Microsoft Corporation.

use std::fmt;

/// The result type for fallible operations that use the [`Error`] type in the `time` module.
pub type Result<T> = std::result::Result<T, Error>;

/// An error that can occur in the `time` module.
///
/// The most common type of error results from overflow, but other errors
/// also exist:
///
/// * Parsing and formatting errors for [`Timestamp`][`super::Timestamp`].
/// * Validation problems.
///
/// # Limited introspection
///
/// Other than implementing the [`std::error::Error`] and [`core::fmt::Debug`] traits, this error type
/// currently provides no introspection capabilities.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use tick::fmt::Iso8601Timestamp;
/// use tick::{Clock, Error, Timestamp};
///
/// "invalid date".parse::<Iso8601Timestamp>().unwrap_err();
/// ```
#[derive(Debug)]
pub struct Error(ErrorKind);

#[derive(Debug)]
enum ErrorKind {
    #[cfg(any(feature = "timestamp", test))]
    Jiff(jiff::Error),
    #[cfg(any(feature = "timestamp", test))]
    OutOfRange(std::borrow::Cow<'static, str>),
    Other(Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl Error {
    const fn from_kind(kind: ErrorKind) -> Self {
        Self(kind)
    }

    #[cfg(any(feature = "timestamp", test))]
    pub(super) fn out_of_range(message: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::from_kind(ErrorKind::OutOfRange(message.into()))
    }

    #[cfg(any(feature = "timestamp", test))]
    pub(super) const fn jiff(error: jiff::Error) -> Self {
        Self::from_kind(ErrorKind::Jiff(error))
    }

    pub(super) fn other(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::from_kind(ErrorKind::Other(Box::new(error)))
    }

    #[cfg(test)]
    const fn kind(&self) -> &ErrorKind {
        &self.0
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            #[cfg(any(feature = "timestamp", test))]
            ErrorKind::Jiff(err) => err.fmt(f),
            #[cfg(any(feature = "timestamp", test))]
            ErrorKind::OutOfRange(msg) => write!(f, "{msg}"),
            ErrorKind::Other(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.0 {
            #[cfg(any(feature = "timestamp", test))]
            ErrorKind::Jiff(err) => Some(err),
            #[cfg(any(feature = "timestamp", test))]
            ErrorKind::OutOfRange(_) => None,
            ErrorKind::Other(err) => Some(err.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use jiff::SignedDuration;

    use super::*;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(Error: Send, Sync);
    }

    #[test]
    fn jiff_error() {
        let error = jiff::Timestamp::from_duration(SignedDuration::MAX).unwrap_err();
        let error = Error::jiff(error);

        assert!(matches!(error.kind(), ErrorKind::Jiff(_)));
        assert_eq!(
            error.to_string(),
            "parameter 'second' with value 9223372036854775807 is not in the required range of -377705023201..=253402207200"
        );
    }

    #[test]
    fn out_of_range_error() {
        let error = Error::out_of_range("test");

        assert!(matches!(error.kind(), ErrorKind::OutOfRange(_)));
        assert_eq!(error.to_string(), "test");
    }

    #[test]
    fn from_other_ok() {
        let error = Error::other(std::io::Error::other("dummy"));

        assert!(matches!(error.kind(), ErrorKind::Other(_)));
        assert_eq!(error.to_string(), "dummy");
        assert_eq!(error.source().unwrap().to_string(), "dummy");
    }
}
