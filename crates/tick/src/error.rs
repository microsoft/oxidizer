// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::time::SystemTimeError;

/// The result type for fallible operations that use the [`Error`] type in the `time` module.
pub type Result<T> = std::result::Result<T, Error>;

/// An error that can occur in the `tick` crate.
///
/// The most common type of error results from overflow, but other errors
/// also exist:
///
/// * Parsing and formatting errors.
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
/// use tick::Error;
/// use tick::fmt::Iso8601;
///
/// "invalid date".parse::<Iso8601>().unwrap_err();
/// ```
#[derive(Debug)]
pub struct Error(ErrorKind);

crate::thread_aware_move!(Error);

#[derive(Debug)]
enum ErrorKind {
    #[cfg(any(feature = "fmt", test))]
    Jiff(jiff::Error),
    #[cfg(any(feature = "fmt", test))]
    OutOfRange(std::borrow::Cow<'static, str>),
    Other(Box<dyn std::error::Error + Send + Sync + 'static>),
    SystemTimeError(SystemTimeError),
}

impl Error {
    const fn from_kind(kind: ErrorKind) -> Self {
        Self(kind)
    }

    #[cfg(any(feature = "fmt", test))]
    pub(super) fn out_of_range(message: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::from_kind(ErrorKind::OutOfRange(message.into()))
    }

    #[cfg(any(feature = "fmt", test))]
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
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::Jiff(err) => err.fmt(f),
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::OutOfRange(msg) => write!(f, "{msg}"),
            ErrorKind::Other(err) => err.fmt(f),
            ErrorKind::SystemTimeError(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.0 {
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::Jiff(err) => Some(err),
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::OutOfRange(_) => None,
            ErrorKind::Other(err) => Some(err.as_ref()),
            ErrorKind::SystemTimeError(err) => Some(err),
        }
    }
}

impl From<SystemTimeError> for Error {
    fn from(err: SystemTimeError) -> Self {
        Self(ErrorKind::SystemTimeError(err))
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use jiff::SignedDuration;

    use super::*;

    #[test]
    fn private_jiff_variant_preserves_message_and_source() {
        // Public format parsing covers construction; variant identity is intentionally private.
        let error = jiff::Timestamp::from_duration(SignedDuration::MAX).unwrap_err();
        let error = Error::jiff(error);

        assert!(matches!(error.kind(), ErrorKind::Jiff(_)));
        assert_eq!(
            error.to_string(),
            "parameter 'Unix timestamp seconds' is not in the required range of -377705023201..=253402207200"
        );
        assert!(error.source().is_some());
    }

    #[test]
    fn private_out_of_range_variant_has_no_source() {
        // The public error type deliberately exposes no variant introspection.
        let error = Error::out_of_range("test");

        assert!(matches!(error.kind(), ErrorKind::OutOfRange(_)));
        assert_eq!(error.to_string(), "test");
        assert!(error.source().is_none());
    }

    #[test]
    fn private_other_variant_preserves_message_and_source() {
        // No public constructor accepts an arbitrary boxed source.
        let error = Error::other(std::io::Error::other("dummy"));

        assert!(matches!(error.kind(), ErrorKind::Other(_)));
        assert_eq!(error.to_string(), "dummy");
        assert_eq!(error.source().unwrap().to_string(), "dummy");
    }
}
