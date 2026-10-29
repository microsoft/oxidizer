// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/recoverable/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/recoverable/favicon.ico")]

//! Recovery classifications for operations.
//!
//! This crate reports whether a recovery attempt can change an outcome.
//! Error types and resilience middleware can share this information.
//! Use [`RecoveryInfo::never`] for successful operations and permanent failures.
//!
//! # Core types
//!
//! - [`RecoveryInfo`] contains a recovery classification and an optional delay.
//! - [`Recovery`] lets a type provide its recovery information.
//! - [`RecoveryKind`] identifies the recovery classification.
//!
//! # Examples
//!
//! ## Classify errors
//!
//! ```rust
//! use recoverable::{Recovery, RecoveryInfo, RecoveryKind};
//!
//! #[derive(Debug)]
//! enum DatabaseError {
//!     ConnectionTimeout,
//!     InvalidCredentials,
//!     TableNotFound,
//! }
//!
//! impl Recovery for DatabaseError {
//!     fn recovery(&self) -> RecoveryInfo {
//!         match self {
//!             // A retry can recover from this transient failure.
//!             DatabaseError::ConnectionTimeout => RecoveryInfo::retry(),
//!             // A retry cannot recover from these permanent failures.
//!             DatabaseError::InvalidCredentials => RecoveryInfo::never(),
//!             DatabaseError::TableNotFound => RecoveryInfo::never(),
//!         }
//!     }
//! }
//!
//! let error = DatabaseError::ConnectionTimeout;
//! assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
//!
//! // Successful operations also use never() because they need no recovery.
//! let success_result: Result<(), DatabaseError> = Ok(());
//! ```
//!
//! ## Set a retry delay
//!
//! Use [`RecoveryInfo::delay`] to specify when to retry an operation.
//!
//! ```rust
//! use std::time::Duration;
//!
//! use recoverable::{RecoveryInfo, RecoveryKind};
//!
//! // Retry after 30 seconds, as specified by a Retry-After header.
//! let recovery = RecoveryInfo::retry().delay(Duration::from_secs(30));
//! assert_eq!(recovery.kind(), RecoveryKind::Retry);
//! assert_eq!(recovery.get_delay(), Some(Duration::from_secs(30)));
//!
//! // Retry immediately.
//! let immediate = RecoveryInfo::retry().delay(Duration::ZERO);
//! assert_eq!(immediate.get_delay(), Some(Duration::ZERO));
//! ```

use std::fmt::{Display, Formatter};
use std::time::Duration;

mod io;

#[cfg(any(doc, test))]
pub mod documentation;

// Naming convention for accessors:
//
// Setters use plain names such as `delay()`. Getters use the `get_` prefix, such
// as `get_delay()`. Callers use setters more often, and the prefix distinguishes
// each getter from its setter.

/// Contains the recovery classification and optional delay for an operation or condition.
///
/// Create a value with [`unknown`](Self::unknown), [`never`](Self::never),
/// [`retry`](Self::retry), or [`unavailable`](Self::unavailable).
/// Use [`delay`](Self::delay) to set a delay.
///
/// # Examples
///
/// ```rust
/// use recoverable::{RecoveryInfo, RecoveryKind};
///
/// let recovery = RecoveryInfo::retry();
/// assert_eq!(recovery.kind(), RecoveryKind::Retry);
/// ```
#[derive(Debug, PartialEq, Clone, Eq, Hash)]
pub struct RecoveryInfo {
    kind: RecoveryKind,
    delay: Option<Duration>,
}

/// Classifies recovery for an operation or condition.
///
/// Use [`RecoveryInfo::kind`] to get this value from [`RecoveryInfo`].
///
/// # Handle unknown variants
///
/// This enum is `#[non_exhaustive]`.
/// A future version can add variants without a major version change. When you
/// match on `RecoveryKind`, include a wildcard arm.
/// Treat unrecognized variants in the same way as [`RecoveryKind::Unknown`].
///
/// ```rust
/// use recoverable::{RecoveryInfo, RecoveryKind};
///
/// fn should_retry(recovery: &RecoveryInfo) -> bool {
///     match recovery.kind() {
///         RecoveryKind::Retry => true,
///         RecoveryKind::Never => false,
///         RecoveryKind::Unavailable => false,
///         // Treat unknown and future variants conservatively.
///         RecoveryKind::Unknown | _ => false,
///     }
/// }
/// ```
///
/// # Examples
///
/// ```rust
/// use recoverable::{RecoveryInfo, RecoveryKind};
///
/// let recovery = RecoveryInfo::unknown();
/// assert_eq!(recovery.kind(), RecoveryKind::Unknown);
/// ```
#[derive(Debug, PartialEq, Clone, Eq, Copy, Hash)]
#[non_exhaustive]
pub enum RecoveryKind {
    /// The application has no known recovery classification.
    ///
    /// Risk requirements determine how the application interprets this classification.
    Unknown,

    /// A recovery attempt can resolve this transient condition.
    ///
    /// Repeated attempts can resolve the condition.
    /// A value from [`RecoveryInfo::get_delay`] specifies the wait before an attempt.
    Retry,

    /// The outcome needs no recovery attempt, or no attempt can change it.
    ///
    /// This classification covers successful operations and permanent failures.
    Never,

    /// The condition is unlikely to recover soon, or its recovery time is uncertain.
    ///
    /// Several recovery attempts can fail before the condition changes.
    Unavailable,
}

impl RecoveryInfo {
    /// Creates recovery information for an unknown condition.
    ///
    /// If the correct recovery classification is not known, use this value.
    /// If the application requires a conservative policy, choose one.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{RecoveryInfo, RecoveryKind};
    ///
    /// let recovery = RecoveryInfo::unknown();
    /// assert_eq!(recovery.kind(), RecoveryKind::Unknown);
    /// ```
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            kind: RecoveryKind::Unknown,
            delay: None,
        }
    }

    /// Creates recovery information for an outcome that needs no recovery.
    ///
    /// Use this value for these outcomes:
    ///
    /// - Successful operations that need no recovery.
    /// - Permanent failures that need user action or a code change.
    ///
    /// Permanent failures include malformed requests, authentication failures,
    /// and missing resources. Recovery information describes recoverability,
    /// not success or failure.
    /// If recovery cannot change the outcome, use `never()`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{RecoveryInfo, RecoveryKind};
    ///
    /// // An authentication failure is permanent.
    /// let auth_failure = RecoveryInfo::never();
    /// assert_eq!(auth_failure.kind(), RecoveryKind::Never);
    ///
    /// // A successful operation needs no recovery.
    /// let success = RecoveryInfo::never();
    /// assert_eq!(success.kind(), RecoveryKind::Never);
    /// assert_eq!(success.get_delay(), None);
    /// ```
    #[must_use]
    pub const fn never() -> Self {
        Self {
            kind: RecoveryKind::Never,
            delay: None,
        }
    }

    /// Creates recovery information for a transient condition.
    ///
    /// Use this value for failures that usually resolve within seconds or minutes.
    /// Examples include network timeouts, brief resource contention, and rate limits.
    /// This value does not include a wait before the next attempt.
    ///
    /// Use [`RecoveryInfo::unavailable`] for service-wide failures that can last longer.
    /// If the service specifies a wait, add it with [`delay`](Self::delay).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{RecoveryInfo, RecoveryKind};
    ///
    /// let recovery = RecoveryInfo::retry();
    /// assert_eq!(recovery.kind(), RecoveryKind::Retry);
    /// assert_eq!(recovery.get_delay(), None);
    /// ```
    #[must_use]
    pub const fn retry() -> Self {
        Self {
            kind: RecoveryKind::Retry,
            delay: None,
        }
    }

    /// Creates recovery information for a condition with slow or uncertain recovery.
    ///
    /// If several recovery attempts can fail before the condition changes, use
    /// this value. Examples include service-wide outages and local resource
    /// failures. [`RecoveryInfo::retry`] indicates that the condition will
    /// usually resolve quickly.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::RecoveryInfo;
    ///
    /// let recovery = RecoveryInfo::unavailable();
    /// ```
    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            kind: RecoveryKind::Unavailable,
            delay: None,
        }
    }

    /// Adds a delay to this recovery information.
    ///
    /// The delay specifies when to try recovery.
    /// Its meaning depends on the recovery kind:
    ///
    /// - For [`RecoveryInfo::retry`], it is the wait before the next attempt.
    ///   For example, the value can come from a `Retry-After` header.
    /// - For [`RecoveryInfo::unavailable`], it is an estimate of the earliest recovery time.
    ///   An attempt before this time will usually fail.
    /// - For other recovery kinds, the delay has no defined meaning, but this
    ///   value preserves it.
    ///
    /// A zero duration specifies an immediate recovery attempt.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use recoverable::{RecoveryInfo, RecoveryKind};
    ///
    /// // The service specifies a retry after 30 seconds.
    /// let recovery = RecoveryInfo::retry().delay(Duration::from_secs(30));
    /// assert_eq!(recovery.kind(), RecoveryKind::Retry);
    /// assert_eq!(recovery.get_delay(), Some(Duration::from_secs(30)));
    ///
    /// // The service provides an estimated recovery time.
    /// let recovery = RecoveryInfo::unavailable().delay(Duration::from_secs(300));
    /// assert_eq!(recovery.kind(), RecoveryKind::Unavailable);
    /// assert_eq!(recovery.get_delay(), Some(Duration::from_secs(300)));
    /// ```
    #[must_use]
    pub const fn delay(self, delay: Duration) -> Self {
        // See the file-level naming comment for the plain setter name.
        Self {
            kind: self.kind,
            delay: Some(delay),
        }
    }

    /// Returns the recovery kind.
    ///
    /// Use this classification to choose a recovery policy.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{RecoveryInfo, RecoveryKind};
    ///
    /// let recovery = RecoveryInfo::unknown();
    /// assert_eq!(recovery.kind(), RecoveryKind::Unknown);
    ///
    /// let recovery = RecoveryInfo::retry();
    /// assert_eq!(recovery.kind(), RecoveryKind::Retry);
    /// ```
    #[must_use]
    pub const fn kind(&self) -> RecoveryKind {
        self.kind
    }

    /// Returns the delay set by [`delay`](Self::delay).
    ///
    /// Use this value with [`kind`](Self::kind). This method returns
    /// `Some(duration)` when a delay is present and `None` otherwise.
    ///
    /// The meaning of the returned delay depends on the recovery kind:
    ///
    /// - For [`RecoveryInfo::retry`], it is the wait before the next attempt.
    /// - For [`RecoveryInfo::unavailable`], it is an estimate of the earliest recovery time.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use recoverable::RecoveryInfo;
    ///
    /// // The service specifies a wait before the next attempt.
    /// let delay = RecoveryInfo::retry().delay(Duration::from_secs(30));
    /// assert_eq!(delay.get_delay(), Some(Duration::from_secs(30)));
    ///
    /// // The service provides an estimated recovery time.
    /// let unavailable_with_time = RecoveryInfo::unavailable().delay(Duration::from_secs(300));
    /// assert_eq!(
    ///     unavailable_with_time.get_delay(),
    ///     Some(Duration::from_secs(300))
    /// );
    /// // The value has no delay.
    /// let no_delay = RecoveryInfo::retry();
    /// assert_eq!(no_delay.get_delay(), None);
    /// ```
    #[must_use]
    pub const fn get_delay(&self) -> Option<Duration> {
        // See the file-level naming comment for the `get_` prefix.
        self.delay
    }
}

/// Provides recovery information for a value.
///
/// Implement this trait for an error or another type that can classify its
/// state. Resilience middleware can then handle different types in the same way.
/// An error usually implements this trait when its variants have different classifications.
///
/// # Examples
///
/// This example implements the trait for an error type.
///
/// ```rust
/// use recoverable::{Recovery, RecoveryInfo};
///
/// #[derive(Debug)]
/// enum DatabaseError {
///     ConnectionTimeout,
///     InvalidCredentials,
///     TableNotFound,
/// }
///
/// impl Recovery for DatabaseError {
///     fn recovery(&self) -> RecoveryInfo {
///         match self {
///             DatabaseError::ConnectionTimeout => RecoveryInfo::retry(),
///             DatabaseError::InvalidCredentials => RecoveryInfo::never(),
///             DatabaseError::TableNotFound => RecoveryInfo::never(),
///         }
///     }
/// }
/// ```
pub trait Recovery {
    /// Returns recovery information for this value.
    ///
    /// The implementation classifies the current state of the value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{Recovery, RecoveryInfo, RecoveryKind};
    ///
    /// struct MyError;
    ///
    /// impl Recovery for MyError {
    ///     fn recovery(&self) -> RecoveryInfo {
    ///         RecoveryInfo::retry()
    ///     }
    /// }
    ///
    /// let error = MyError;
    /// assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
    /// ```
    fn recovery(&self) -> RecoveryInfo;
}

impl<R, E> Recovery for Result<R, E>
where
    R: Recovery,
    E: Recovery,
{
    fn recovery(&self) -> RecoveryInfo {
        match self {
            Ok(res) => res.recovery(),
            Err(err) => err.recovery(),
        }
    }
}

impl Display for RecoveryInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(delay) = self.delay {
            return write!(f, "{} (delay {:?})", self.kind, delay);
        }

        Display::fmt(&self.kind, f)
    }
}

impl RecoveryKind {
    /// Returns the name of this recovery kind.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::RecoveryKind;
    ///
    /// assert_eq!(RecoveryKind::Retry.as_str(), "retry");
    /// assert_eq!(RecoveryKind::Never.as_str(), "never");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Never => "never",
            Self::Retry => "retry",
            Self::Unavailable => "unavailable",
        }
    }
}

impl Display for RecoveryKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use static_assertions::{assert_impl_all, assert_not_impl_all};

    use super::*;

    assert_impl_all!(RecoveryInfo: Debug, PartialEq, Clone, Send, Sync, Eq, PartialEq);
    assert_impl_all!(RecoveryKind: Debug, PartialEq, Clone, Eq, Copy, std::hash::Hash);

    // Keep this type extensible for fields that do not implement Copy.
    assert_not_impl_all!(RecoveryInfo: Copy);

    #[test]
    fn recovery_enum() {
        assert_eq!(RecoveryInfo::unknown().kind(), RecoveryKind::Unknown);
        assert_eq!(RecoveryInfo::unavailable().kind(), RecoveryKind::Unavailable);
        assert_eq!(RecoveryInfo::retry().kind(), RecoveryKind::Retry);
        assert_eq!(RecoveryInfo::retry().delay(Duration::ZERO).kind(), RecoveryKind::Retry);
        assert_eq!(RecoveryInfo::never().kind(), RecoveryKind::Never);
    }

    #[test]
    fn display_ok() {
        assert_eq!(RecoveryInfo::unknown().to_string(), "unknown");
        assert_eq!(RecoveryInfo::never().to_string(), "never");
        assert_eq!(RecoveryInfo::retry().to_string(), "retry");
        assert_eq!(RecoveryInfo::unavailable().to_string(), "unavailable");
        assert_eq!(
            RecoveryInfo::retry().delay(Duration::from_secs(30)).to_string(),
            "retry (delay 30s)"
        );
        assert_eq!(
            RecoveryInfo::unavailable().delay(Duration::from_mins(5)).to_string(),
            "unavailable (delay 300s)"
        );
    }

    #[test]
    fn recovery_kind_as_str() {
        assert_eq!(RecoveryKind::Unknown.as_str(), "unknown");
        assert_eq!(RecoveryKind::Never.as_str(), "never");
        assert_eq!(RecoveryKind::Retry.as_str(), "retry");
        assert_eq!(RecoveryKind::Unavailable.as_str(), "unavailable");
    }

    #[test]
    fn recovery_kind_display_ok() {
        assert_eq!(RecoveryKind::Unknown.to_string(), "unknown");
        assert_eq!(RecoveryKind::Never.to_string(), "never");
        assert_eq!(RecoveryKind::Retry.to_string(), "retry");
        assert_eq!(RecoveryKind::Unavailable.to_string(), "unavailable");
    }

    #[test]
    fn delay_behavior() {
        let thirty_seconds = Duration::from_secs(30);
        let recovery = RecoveryInfo::retry().delay(thirty_seconds);

        assert_eq!(recovery.get_delay(), Some(thirty_seconds));
        assert_eq!(recovery.kind(), RecoveryKind::Retry);

        // A zero duration is preserved.
        let zero_duration = RecoveryInfo::retry().delay(Duration::ZERO);
        assert_eq!(zero_duration.get_delay(), Some(Duration::ZERO));

        // A delay can apply to any recovery kind.
        let unavailable = RecoveryInfo::unavailable().delay(Duration::from_mins(5));
        assert_eq!(unavailable.get_delay(), Some(Duration::from_mins(5)));
        assert_eq!(unavailable.kind(), RecoveryKind::Unavailable);

        // A new delay replaces the previous delay.
        let updated = RecoveryInfo::retry().delay(Duration::from_secs(10)).delay(Duration::from_secs(20));
        assert_eq!(updated.get_delay(), Some(Duration::from_secs(20)));
    }

    #[test]
    fn unavailable_behavior() {
        let recovery = RecoveryInfo::unavailable();
        assert_eq!(recovery.get_delay(), None);

        let recovery = RecoveryInfo::unavailable().delay(Duration::ZERO);
        assert_eq!(recovery.get_delay(), Some(Duration::ZERO));

        let recovery = RecoveryInfo::unavailable().delay(Duration::from_secs(1));
        assert_eq!(recovery.get_delay(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn assert_result_implements_recover() {
        assert_impl_all!(Result<TestType, TestType>: Recovery);
        assert_not_impl_all!(Result<TestType, String>: Recovery);
    }

    #[test]
    fn get_delay_ok() {
        assert_eq!(RecoveryInfo::unknown().get_delay(), None);
        assert_eq!(RecoveryInfo::never().get_delay(), None);
        assert_eq!(RecoveryInfo::retry().get_delay(), None);
        assert_eq!(
            RecoveryInfo::retry().delay(Duration::from_mins(1)).get_delay(),
            Some(Duration::from_mins(1))
        );
        assert_eq!(RecoveryInfo::unavailable().get_delay(), None);
        assert_eq!(
            RecoveryInfo::unavailable().delay(Duration::from_mins(5)).get_delay(),
            Some(Duration::from_mins(5))
        );
    }

    #[test]
    fn recover_trait_implementations() {
        assert_eq!(
            (Ok(TestType) as Result<TestType, TestType>).recovery().kind(),
            RecoveryKind::Unknown
        );
        assert_eq!(
            (Err(TestType) as Result<TestType, TestType>).recovery().kind(),
            RecoveryKind::Unknown
        );
    }

    // Result implements Recovery when both variants implement it.
    #[derive(Debug)]
    struct TestType;
    impl Recovery for TestType {
        fn recovery(&self) -> RecoveryInfo {
            RecoveryInfo::unknown()
        }
    }
}
