// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/recoverable/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/recoverable/favicon.ico")]

//! Recovery information and classification for resilience patterns.
//!
//! This crate provides types for classifying conditions based on their **recoverability state**,
//! enabling consistent recovery behavior across different error types and resilience middleware.
//!
//! Recovery information describes whether retrying an operation might help, not whether the
//! operation succeeded or failed. Both successful operations and permanent failures should use
//! [`RecoveryInfo::never`] since recovery is not necessary or desirable.
//!
//! # Core Types
//!
//! - [`RecoveryInfo`]: Classifies conditions as recoverable (transient) or non-recoverable (permanent/successful).
//! - [`Recovery`]: A trait for types that can provide their own recovery information.
//! - [`RecoveryKind`]: An enum representing the kind of recovery that can be attempted.
//!
//! # Examples
//!
//! ## Classifying Errors
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
//!             // Transient failure - might succeed if retried
//!             DatabaseError::ConnectionTimeout => RecoveryInfo::retry(),
//!             // Permanent failures - retrying won't help
//!             DatabaseError::InvalidCredentials => RecoveryInfo::never(),
//!             DatabaseError::TableNotFound => RecoveryInfo::never(),
//!         }
//!     }
//! }
//!
//! let error = DatabaseError::ConnectionTimeout;
//! assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
//!
//! // Successful operations also use never() since retry is unnecessary
//! let success_result: Result<(), DatabaseError> = Ok(());
//! ```
//!
//! ## Retry Delay
//!
//! You can specify when to retry an operation using the `delay` method:
//!
//! ```rust
//! use std::time::Duration;
//!
//! use recoverable::{RecoveryInfo, RecoveryKind};
//!
//! // Retry with a 30-second delay (e.g., from a Retry-After header)
//! let recovery = RecoveryInfo::retry().delay(Duration::from_secs(30));
//! assert_eq!(recovery.kind(), RecoveryKind::Retry);
//! assert_eq!(recovery.get_delay(), Some(Duration::from_secs(30)));
//!
//! // Immediate retry
//! let immediate = RecoveryInfo::retry().delay(Duration::ZERO);
//! assert_eq!(immediate.get_delay(), Some(Duration::ZERO));
//! ```

use std::fmt::{Display, Formatter};
use std::time::Duration;

mod io;

pub mod _documentation;

// Naming Convention for Get/Set:
//
// This type uses an unconventional naming pattern where setters use plain names (e.g., `delay()`)
// and getters use the `get_` prefix (e.g., `get_delay()`). This deviates from standard Rust
// conventions because setters are used much more frequently than getters in typical usage patterns.
// The `get_` prefix on getters helps distinguish them from their corresponding setters.

/// Recovery information associated with an operation or condition.
///
/// Describes how an operation can be recovered from, if at all. Construct instances via the
/// named constructors [`unknown`](Self::unknown), [`never`](Self::never),
/// [`retry`](Self::retry), and [`unavailable`](Self::unavailable), and optionally attach a
/// delay hint with [`delay`](Self::delay).
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

/// Kind of recovery that can be attempted.
///
/// Retrieve the recovery kind from a [`RecoveryInfo`] instance via [`RecoveryInfo::kind`].
///
/// # Handling Unknown Variants
///
/// This enum is `#[non_exhaustive]`, so new variants may be added in future versions without
/// a major version bump. When matching on `RecoveryKind`, always include a wildcard arm that
/// treats unrecognized variants the same as [`RecoveryKind::Unknown`]:
///
/// ```rust
/// use recoverable::{RecoveryInfo, RecoveryKind};
///
/// fn should_retry(recovery: &RecoveryInfo) -> bool {
///     match recovery.kind() {
///         RecoveryKind::Retry => true,
///         RecoveryKind::Never => false,
///         RecoveryKind::Unavailable => false,
///         // Treat unknown (and any future variants) conservatively
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
    /// The recoverability of the condition is unknown.
    ///
    /// Handling should be determined on a case-by-case basis. For example,
    /// unclassified network errors might warrant retrying. Consider an
    /// optimistic or pessimistic approach based on your application's requirements.
    Unknown,

    /// The condition is transient and may resolve with recovery.
    ///
    /// Retry the operation with backoff, respecting any [`RecoveryInfo::delay`] hint.
    Retry,

    /// The condition is permanent and recovery won't help.
    ///
    /// Do not retry; propagate the error as a terminal failure.
    Never,

    /// Service-wide unavailability or significant degradation.
    ///
    /// Retrying has a low chance of success. Consider circuit-breaker patterns,
    /// failing fast, or falling back to cached data.
    Unavailable,
}

impl RecoveryInfo {
    /// Creates recovery info for an unknown condition.
    ///
    /// Use when it is unclear whether recovery would help. Consider treating
    /// unknown conditions conservatively based on your application's requirements.
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

    /// Creates recovery info for a permanent, non-recoverable condition.
    ///
    /// Use for both successful operations and permanent failures:
    ///
    /// - **Successful operations**: The operation completed successfully; no recovery needed.
    /// - **Permanent failures**: Malformed requests, authentication failures, resource not found,
    ///   or other errors that require user intervention or code changes to resolve.
    ///
    /// Recovery information describes **recoverability state**, not success/failure status.
    /// If recovery would not change the outcome, use `never()` regardless of whether the
    /// original operation succeeded or failed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{RecoveryInfo, RecoveryKind};
    ///
    /// // Permanent failure - authentication failed
    /// let auth_failure = RecoveryInfo::never();
    /// assert_eq!(auth_failure.kind(), RecoveryKind::Never);
    ///
    /// // Successful operation - also uses never() since recovery is unnecessary
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

    /// Creates recovery info for a transient, likely recoverable condition.
    ///
    /// Use for transient failures that are expected to resolve relatively quickly,
    /// such as network timeouts, brief resource contention, or rate limiting.
    /// These conditions typically resolve within seconds to minutes without any
    /// specific timing guidance from the service.
    ///
    /// For service-wide unavailability that may take much longer to resolve,
    /// use [`RecoveryInfo::unavailable`] instead. For cases where the service provides
    /// explicit timing guidance, chain the [`delay`](Self::delay) method.
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

    /// Creates recovery info for service-wide unavailability or significant degradation.
    ///
    /// Use when the failure is due to a service-wide issue that affects many users and may
    /// take an extended period to resolve (minutes to hours). Unlike [`RecoveryInfo::retry`],
    /// which suggests quick resolution, unavailability indicates uncertainty about recovery
    /// timing and suggests that multiple recovery attempts may fail before the service recovers.
    ///
    /// To attach a recovery delay hint, chain the [`delay`](Self::delay) method:
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use recoverable::RecoveryInfo;
    ///
    /// let recovery = RecoveryInfo::unavailable().delay(Duration::from_secs(300));
    /// ```
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

    /// Attaches a delay hint to this recovery information.
    ///
    /// The delay indicates when a recovery attempt should be made. Its meaning depends on
    /// the recovery kind:
    ///
    /// - For [`RecoveryInfo::retry`]: High-confidence timing guidance (e.g., from a
    ///   `Retry-After` header) indicating when the recovery attempt is likely to succeed.
    /// - For [`RecoveryInfo::unavailable`]: Low-confidence estimate for the earliest time
    ///   when recovery attempts might succeed. Attempts before this time are expected to fail.
    /// - For other recovery kinds: Generally not applicable, but the delay will be preserved.
    ///
    /// If the duration is zero, it's a hint to attempt recovery immediately.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use recoverable::{RecoveryInfo, RecoveryKind};
    ///
    /// // Service indicates to retry after 30 seconds
    /// let recovery = RecoveryInfo::retry().delay(Duration::from_secs(30));
    /// assert_eq!(recovery.kind(), RecoveryKind::Retry);
    /// assert_eq!(recovery.get_delay(), Some(Duration::from_secs(30)));
    ///
    /// // Unavailability with recovery estimate
    /// let recovery = RecoveryInfo::unavailable().delay(Duration::from_secs(300));
    /// assert_eq!(recovery.kind(), RecoveryKind::Unavailable);
    /// assert_eq!(recovery.get_delay(), Some(Duration::from_secs(300)));
    /// ```
    #[must_use]
    pub const fn delay(self, delay: Duration) -> Self {
        // See file-level "Naming Convention" comment for why this uses a plain name.
        Self {
            kind: self.kind,
            delay: Some(delay),
        }
    }

    /// Returns the recovery kind for this instance.
    ///
    /// Use this to determine the appropriate recovery strategy.
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

    /// Returns the delay hint, if one was set via [`delay`](Self::delay).
    ///
    /// Use together with [`kind`](Self::kind) to determine both whether a condition is
    /// recoverable and whether an explicit delay is provided. Returns `Some(duration)` when
    /// a delay has been set, and `None` otherwise.
    ///
    /// The meaning of the returned delay depends on the recovery kind:
    /// - For [`RecoveryInfo::retry`]: High-confidence timing guidance indicating when recovery will likely succeed.
    /// - For [`RecoveryInfo::unavailable`]: Low-confidence estimate for the earliest time when recovery might succeed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use recoverable::RecoveryInfo;
    ///
    /// // Specific delay requested with high confidence of success
    /// let delay = RecoveryInfo::retry().delay(Duration::from_secs(30));
    /// assert_eq!(delay.get_delay(), Some(Duration::from_secs(30)));
    ///
    /// // No delay specified
    /// let immediate = RecoveryInfo::retry();
    /// assert_eq!(immediate.get_delay(), None);
    ///
    /// // Unavailability with no recovery estimate
    /// let unavailable = RecoveryInfo::unavailable();
    /// assert_eq!(unavailable.get_delay(), None);
    ///
    /// // Unavailability with low-confidence recovery estimate
    /// let unavailable_with_time = RecoveryInfo::unavailable().delay(Duration::from_secs(300));
    /// assert_eq!(
    ///     unavailable_with_time.get_delay(),
    ///     Some(Duration::from_secs(300))
    /// );
    ///
    /// // Non-recoverable
    /// let never = RecoveryInfo::never();
    /// assert_eq!(never.get_delay(), None);
    /// ```
    #[must_use]
    pub const fn get_delay(&self) -> Option<Duration> {
        // See file-level "Naming Convention" comment for why this uses the `get_` prefix.
        self.delay
    }
}

/// Enables types to provide their own recovery information.
///
/// Implement this trait for errors or any type that can indicate the recoverability of
/// its state. This allows consistent handling of recoverable conditions across various
/// types in resilience middleware.
///
/// Typical implementers are error types whose variants can be classified as transient or
/// permanent, depending on the specific error condition.
///
/// # Examples
///
/// Basic implementation for a simple error type:
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
    /// Returns recovery information for this instance.
    ///
    /// Implementations should return appropriate recovery information based on the
    /// internal state of the type.
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
    /// Returns a string representation of this recovery kind.
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

    // cannot be Copy because in the future we may want to add more fields that are not Copy
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
            RecoveryInfo::unavailable().delay(Duration::from_secs(300)).to_string(),
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

        // Zero duration
        let zero_duration = RecoveryInfo::retry().delay(Duration::ZERO);
        assert_eq!(zero_duration.get_delay(), Some(Duration::ZERO));

        // Delay can be applied to any recovery kind
        let unavailable = RecoveryInfo::unavailable().delay(Duration::from_secs(300));
        assert_eq!(unavailable.get_delay(), Some(Duration::from_secs(300)));
        assert_eq!(unavailable.kind(), RecoveryKind::Unavailable);

        // Applying delay multiple times replaces the previous delay
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
            RecoveryInfo::retry().delay(Duration::from_secs(60)).get_delay(),
            Some(Duration::from_secs(60))
        );
        assert_eq!(RecoveryInfo::unavailable().get_delay(), None);
        assert_eq!(
            RecoveryInfo::unavailable().delay(Duration::from_secs(300)).get_delay(),
            Some(Duration::from_secs(300))
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

    // Result implements Recovery
    #[derive(Debug)]
    struct TestType;
    impl Recovery for TestType {
        fn recovery(&self) -> RecoveryInfo {
            RecoveryInfo::unknown()
        }
    }
}
