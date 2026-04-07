// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Error types for cache operations.

use std::error::Error as StdError;

use ohno::OhnoCore;
use recoverable::{Recovery, RecoveryInfo};

/// An error from a cache operation.
///
/// Wraps any underlying error from a cache implementation while preserving
/// the ability to extract the original typed error.
///
/// # For `CacheTier` implementers
///
/// Wrap your storage-specific errors using [`from_source`](Self::from_source):
///
/// ```ignore
/// impl CacheTier<K, V> for RedisCache {
///     async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
///         self.client.get(key).await.map_err(Error::from_source)
///     }
/// }
/// ```
///
/// # For Consumers
///
/// Extract the underlying error using [`source_as`](Self::source_as) or
/// [`find_source`](ohno::ErrorExt::find_source):
///
/// ```ignore
/// match cache.get(&key).await {
///     Err(e) if e.is_source::<redis::RedisError>() => {
///         let redis_err = e.source_as::<redis::RedisError>().unwrap();
///         // Handle Redis-specific error
///     }
///     Err(e) => // Handle generic error
///     Ok(v) => // Success
/// }
/// ```
#[ohno::error]
#[no_constructors]
#[derive(Clone)]
pub struct Error {
    recovery_info: RecoveryInfo,
}

impl Error {
    fn caused_by(cause: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self {
            ohno_core: OhnoCore::from(cause),
            recovery_info: RecoveryInfo::never(),
        }
    }

    /// Creates a new error wrapping a source error.
    ///
    /// This preserves the original error type for later extraction via
    /// [`source_as`](Self::source_as).
    ///
    /// # Examples
    ///
    /// ```
    /// use cachet_tier::Error;
    ///
    /// let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
    /// let error = Error::from_source(io_err);
    ///
    /// // Later, extract the original error
    /// assert!(error.source_as::<std::io::Error>().is_some());
    /// ```
    pub fn from_source(cause: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self::caused_by(cause)
    }

    /// Creates a new error from a message string.
    ///
    /// Use [`from_source`](Self::from_source) instead when wrapping an existing error.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachet_tier::Error;
    ///
    /// let error = Error::from_message("operation failed");
    /// ```
    pub fn from_message(message: impl Into<String>) -> Self {
        Self::caused_by(message.into())
    }

    /// Attaches recovery information to this error.
    ///
    /// This is for informational purposes and does not affect error handling
    /// logic. It can be used by monitoring or debugging tools to provide
    /// hints on how to recover from the error.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachet_tier::Error;
    /// use recoverable::RecoveryInfo;
    ///
    /// let error = Error::from_message("temporary failure").with_recovery(RecoveryInfo::retry());
    /// ```
    #[must_use]
    pub fn with_recovery(self, recovery_info: RecoveryInfo) -> Self {
        Self { recovery_info, ..self }
    }

    /// Returns `true` if the source error is of type `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachet_tier::Error;
    ///
    /// let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
    /// let error = Error::from_source(io_err);
    ///
    /// assert!(error.is_source::<std::io::Error>());
    /// assert!(!error.is_source::<std::fmt::Error>());
    /// ```
    #[must_use]
    pub fn is_source<T: StdError + 'static>(&self) -> bool {
        self.source_as::<T>().is_some()
    }

    /// Returns the source error as type `T` if it matches.
    ///
    /// This checks the immediate source. For nested errors, use
    /// [`find_source`](ohno::ErrorExt::find_source) from the `ErrorExt` trait.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachet_tier::Error;
    ///
    /// let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
    /// let error = Error::from_source(io_err);
    ///
    /// if let Some(io_err) = error.source_as::<std::io::Error>() {
    ///     assert_eq!(io_err.kind(), std::io::ErrorKind::NotFound);
    /// }
    /// ```
    #[must_use]
    pub fn source_as<T: StdError + 'static>(&self) -> Option<&T> {
        self.source().and_then(|s| s.downcast_ref::<T>())
    }
}

impl Recovery for Error {
    fn recovery(&self) -> RecoveryInfo {
        self.recovery_info.clone()
    }
}

impl From<LenError> for Error {
    fn from(err: LenError) -> Self {
        Self::from_source(err)
    }
}

/// The kind of error returned by [`CacheTier::len`](crate::CacheTier::len).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LenErrorKind {
    /// The tier does not support reporting its size.
    Unsupported,
    /// The underlying storage operation failed.
    Failed,
}

/// An error from a [`CacheTier::len`](crate::CacheTier::len) operation.
///
/// Use the [`kind`](Self::kind) field to distinguish between tiers that don't
/// support size reporting ([`Unsupported`](LenErrorKind::Unsupported)) and
/// actual storage failures ([`Failed`](LenErrorKind::Failed)).
#[ohno::error]
#[from(Error(kind: LenErrorKind::Failed))]
pub struct LenError {
    /// The kind of error that occurred.
    pub kind: LenErrorKind,
}

impl LenError {
    /// Creates a new `LenError` indicating that the tier does not support size reporting.
    #[must_use]
    pub fn unsupported() -> Self {
        Self::new(LenErrorKind::Unsupported)
    }
}

/// A specialized [`Result`] type for cache operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use std::io::{self, ErrorKind};

    use super::*;

    #[test]
    fn error_debug_contains_cause_message() {
        let error = Error::from_message("test error message");
        let debug_str = format!("{error:?}");
        assert!(
            debug_str.contains("test error message"),
            "debug output should contain the cause message, got: {debug_str}"
        );
    }

    #[test]
    fn error_display_contains_cause_message() {
        let error = Error::from_message("display test");
        let display_str = format!("{error}");
        assert!(
            display_str.contains("display test"),
            "display output should contain the cause message, got: {display_str}"
        );
    }

    #[test]
    fn result_type_alias_propagates_errors() {
        fn returns_err() -> Result<i32> {
            Err(Error::from_message("expected failure"))
        }

        let err = returns_err().expect_err("should return an error");
        assert!(format!("{err}").contains("expected failure"));
    }

    #[test]
    fn from_source_preserves_error_type() {
        let io_err = io::Error::new(ErrorKind::ConnectionRefused, "connection refused");
        let error = Error::from_source(io_err);

        assert!(error.is_source::<io::Error>());
        let extracted = error.source_as::<io::Error>().expect("should extract io::Error");
        assert_eq!(extracted.kind(), ErrorKind::ConnectionRefused);
    }

    #[test]
    fn is_source_returns_false_for_wrong_type() {
        let io_err = io::Error::new(ErrorKind::NotFound, "not found");
        let error = Error::from_source(io_err);

        assert!(error.is_source::<io::Error>());
        assert!(!error.is_source::<std::fmt::Error>());
    }

    #[test]
    fn source_as_returns_none_for_wrong_type() {
        let io_err = io::Error::new(ErrorKind::NotFound, "not found");
        let error = Error::from_source(io_err);

        assert!(error.source_as::<io::Error>().is_some());
        assert!(error.source_as::<std::fmt::Error>().is_none());
    }

    #[test]
    fn source_as_returns_none_for_message_only_error() {
        let error = Error::from_message("just a message");

        assert!(!error.is_source::<io::Error>());
        assert!(error.source_as::<io::Error>().is_none());
    }

    #[test]
    fn error_is_clone() {
        let io_err = io::Error::new(ErrorKind::TimedOut, "timeout");
        let error = Error::from_source(io_err);
        let cloned = error.clone();

        // Both should have the same source type
        assert!(error.is_source::<io::Error>());
        assert!(cloned.is_source::<io::Error>());

        // Both should display the same message
        assert_eq!(error.to_string(), cloned.to_string());
    }

    #[test]
    fn error_extract_and_match_on_kind() {
        let io_err = io::Error::new(ErrorKind::PermissionDenied, "access denied");
        let error = Error::from_source(io_err);

        assert_eq!(
            error.source_as::<io::Error>().map(io::Error::kind),
            Some(ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn with_recovery_attaches_recovery_info() {
        let error = Error::from_message("temporary failure").with_recovery(RecoveryInfo::retry());
        assert_eq!(error.recovery(), RecoveryInfo::retry());
    }

    #[test]
    fn recovery_returns_never_by_default() {
        let error = Error::from_message("permanent failure");
        assert_eq!(error.recovery(), RecoveryInfo::never());
    }

    #[test]
    fn failed_lenerror_from_error() {
        let error = Error::from_message("permanent failure");
        let len_err: LenError = error.clone().into();
        assert_eq!(LenErrorKind::Failed, len_err.kind);
        assert_eq!(error.to_string(), len_err.source().expect("should have source error").to_string());
    }

    #[test]
    fn unsupported_lenerror_has_unsupported_kind() {
        let len_err = LenError::unsupported();
        assert_eq!(LenErrorKind::Unsupported, len_err.kind);
    }

    #[test]
    fn lenerror_converts_to_error() {
        let len_err = LenError::unsupported();
        let error: Error = len_err.into();
        assert!(error.is_source::<LenError>());
    }
}
