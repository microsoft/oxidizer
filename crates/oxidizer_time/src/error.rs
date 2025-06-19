// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::time::SystemTimeError;

/// The result for fallible operations that use the [`Error`] type in the `time` module.
pub type Result<T> = std::result::Result<T, Error>;

/// An error that can occur in the `time` module.
///
/// The most common type of error is a result of overflow. But other errors
/// exist as well:
///
/// * Parsing and formatting errors for [`Timestamp`][`super::Timestamp`].
/// * Validation problems.
///
/// # Introspection is limited
///
/// Other than implementing the [`std::error::Error`] and [`core::fmt::Debug`] trait, this error type
/// currently provides no introspection capabilities.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxidizer_time::{Clock, Error, Timestamp};
/// use oxidizer_time::fmt::Iso8601Timestamp;
///
/// "invalid date".parse::<Iso8601Timestamp>().unwrap_err();
/// ```
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct Error(#[from] ErrorKind);

#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    #[error(transparent)]
    Jiff(#[from] jiff::Error),

    #[error(transparent)]
    SystemTime(#[from] SystemTimeError),

    #[error("{0}")]
    OutOfRange(Cow<'static, str>),

    #[error("{0}")]
    Other(Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl Error {
    pub(super) const fn from_kind(kind: ErrorKind) -> Self {
        Self(kind)
    }

    pub(super) fn out_of_range(message: impl Into<Cow<'static, str>>) -> Self {
        Self::from_kind(ErrorKind::OutOfRange(message.into()))
    }

    pub(super) const fn from_jiff(error: jiff::Error) -> Self {
        Self::from_kind(ErrorKind::Jiff(error))
    }

    pub(super) fn from_other(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::from_kind(ErrorKind::Other(Box::new(error)))
    }

    #[cfg(test)]
    pub(super) const fn kind(&self) -> &ErrorKind {
        &self.0
    }
}

impl From<SystemTimeError> for Error {
    fn from(error: SystemTimeError) -> Self {
        Self::from_kind(ErrorKind::SystemTime(error))
    }
}

#[cfg(test)]
mod tests {
    use jiff::SignedDuration;

    use super::*;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(Error: Send, Sync);
    }

    #[test]
    fn jiff_error() {
        let error = jiff::Timestamp::from_duration(SignedDuration::MAX).unwrap_err();
        let error = Error::from_jiff(error);

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
}