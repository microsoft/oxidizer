// Copyright (c) Microsoft Corporation.

//! Recovery metadata and classification for resilience patterns.
//!
//! This crate provides types for classifying conditions based on their **recoverability state**,
//! enabling consistent recovery behavior across different error types and resilience middleware.
//!
//! The recovery metadata describes whether recovering from an operation might help, not whether
//! the operation succeeded or failed. Both successful operations and permanent failures
//! should use [`Recovery::never()`] since recovery won't change the outcome.
//!
//! # Core Types
//!
//! - [`Recovery`]: Classifies conditions as recoverable (transient) or non-recoverable (permanent/successful).
//! - [`Recover`]: A trait for types that can determine their recoverability.
//! - [`RecoveryKind`]: An enum representing the kind of recovery that can be attempted.
//!
//! # Examples
//!
//! ```rust
//! use recoverable::{Recover, Recovery, RecoveryKind};
//!
//! #[derive(Debug)]
//! enum DatabaseError {
//!     ConnectionTimeout,
//!     InvalidCredentials,
//!     TableNotFound,
//! }
//!
//! impl Recover for DatabaseError {
//!     fn recovery(&self) -> Recovery {
//!         match self {
//!             // Transient failure - might succeed if retried
//!             DatabaseError::ConnectionTimeout => Recovery::retry(),
//!             // Permanent failures - retrying won't help
//!             DatabaseError::InvalidCredentials => Recovery::never(),
//!             DatabaseError::TableNotFound => Recovery::never(),
//!         }
//!     }
//! }
//!
//! let error = DatabaseError::ConnectionTimeout;
//! assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
//!
//! // For successful operations, also use never() since retry is unnecessary
//! let success_result: Result<(), DatabaseError> = Ok(());
//! // If we had a wrapper type for success, it would also return Recovery::never()
//! ```

use std::fmt::{Display, Formatter};
use std::time::Duration;

/// Represents the recoverability metadata associated with an operation or condition.
///
/// This type describes how the operation can be recovered from, if at all. It provides
/// various ways to create recovery metadata for different scenarios, such as unknown conditions,
/// permanent failures, transient failures, and service unavailability.
///
/// # Examples
///
/// ```rust
/// use recoverable::{Recovery, RecoveryKind};
///
/// let recovery = Recovery::retry();
/// assert_eq!(recovery.kind(), RecoveryKind::Retry);
/// ```
#[derive(Debug, PartialEq, Clone)]
#[non_exhaustive]
pub struct Recovery {
    kind: RecoveryKind,
    delay: Option<Duration>,
}

/// Represents the kind of recovery that can be attempted.
///
/// To retrieve the recovery kind from a `Recovery` instance, use the [`Recovery::kind`] method.
///
/// # Examples
///
/// ```rust
/// use recoverable::{Recovery, RecoveryKind};
///
/// let recovery = Recovery::unknown();
/// assert_eq!(recovery.kind(), RecoveryKind::Unknown);
/// ```
#[derive(Debug, PartialEq, Clone, Eq, Copy, Hash)]
#[non_exhaustive]
pub enum RecoveryKind {
    /// The condition is unknown.
    Unknown,

    /// The condition is temporary and may resolve quickly with recovery.
    Retry,

    /// The condition is permanent and recovery won't help.
    Never,

    /// Indicates a service-wide unavailability or significant degradation.
    Unavailable,
}

impl Recovery {
    /// Recovery cannot be determined.
    ///
    /// Use when it's unclear whether recovery would help. Consider treating
    /// unknown conditions conservatively based on your application's requirements.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{Recovery, RecoveryKind};
    ///
    /// let recovery = Recovery::unknown();
    /// assert_eq!(recovery.kind(), RecoveryKind::Unknown);
    /// ```
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            kind: RecoveryKind::Unknown,
            delay: None,
        }
    }

    /// The condition is permanent and recovery won't help.
    ///
    /// Use this for both successful operations and permanent failures:
    ///
    /// - **Successful operations**: The operation completed successfully, no recovery needed.
    /// - **Permanent failures**: Malformed requests, authentication failures, resource not found,
    ///   or other errors that require user intervention or code changes to resolve.
    ///
    /// The recovery metadata describes **recoverability state**, not success/failure status.
    /// If recovery doesn't change the outcome, use [`Recovery::never`] regardless of whether the
    /// original operation succeeded or failed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{Recovery, RecoveryKind};
    ///
    /// // Permanent failure - authentication failed
    /// let auth_failure = Recovery::never();
    /// assert_eq!(auth_failure.kind(), RecoveryKind::Never);
    ///
    /// // Successful operation - also uses never() since recovery is unnecessary
    /// let success = Recovery::never();
    /// assert_eq!(success.kind(), RecoveryKind::Never);
    /// assert_eq!(success.recovery_delay(), None);
    /// ```
    #[must_use]
    pub const fn never() -> Self {
        Self {
            kind: RecoveryKind::Never,
            delay: None,
        }
    }

    /// The condition is temporary and may resolve quickly with recovery.
    ///
    /// Use for transient failures that are expected to resolve relatively quickly,
    /// such as network timeouts, brief resource contention, or rate limiting.
    /// These conditions typically resolve within seconds to minutes without any
    /// specific timing guidance from the service.
    ///
    /// For service-wide unavailability that may take much longer to resolve,
    /// use [`Recovery::unavailable`] instead. For cases where the service provides
    /// explicit timing guidance, use the [`Recovery::delay`] method.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{Recovery, RecoveryKind};
    ///
    /// let recovery = Recovery::retry();
    /// assert_eq!(recovery.kind(), RecoveryKind::Retry);
    /// assert_eq!(recovery.recovery_delay(), None);
    /// ```
    #[must_use]
    pub const fn retry() -> Self {
        Self {
            kind: RecoveryKind::Retry,
            delay: None,
        }
    }

    /// Indicates a service is experiencing a widespread unavailability or significant degradation.
    ///
    /// Use when the failure is due to a service-wide unavailability that affects many users
    /// and may take an extended period to resolve (minutes to hours). Unlike
    /// [`Recovery::retry`] which suggests quick resolution, unavailability indicates
    /// uncertainty about recovery timing and suggests that multiple recovery attempts may
    /// fail before the service recovers.
    ///
    /// To specify a recovery delay hint, use the [`Recovery::delay`] method:
    /// ```rust
    /// use std::time::Duration;
    /// use recoverable::Recovery;
    ///
    /// let recovery = Recovery::unavailable().delay(Duration::from_secs(300));
    /// ```
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::Recovery;
    ///
    /// let recovery = Recovery::unavailable();
    /// ```
    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            kind: RecoveryKind::Unavailable,
            delay: None,
        }
    }

    /// Adds a delay hint to this recovery.
    ///
    /// This method associates the duration of the recovery to indicate when a recovery attempt
    /// might succeed. The meaning of the delay depends on the recovery kind:
    ///
    /// - For [`Recovery::retry`]: High-confidence timing guidance (e.g., from a
    ///   `Retry-After` header) indicating when the recovery attempt is likely to succeed.
    /// - For [`Recovery::unavailable`]: Low-confidence estimate for the earliest time
    ///   when recovery attempts might succeed. Attempts before this time are expected to fail.
    /// - For other recovery kinds: Generally not applicable, but the delay will be preserved.
    ///
    /// If the duration is zero, it's a hint to attempt recovery immediately.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    /// use recoverable::{Recovery, RecoveryKind};
    ///
    /// // Service indicates to retry after 30 seconds
    /// let recovery = Recovery::retry().delay(Duration::from_secs(30));
    /// assert_eq!(recovery.kind(), RecoveryKind::Retry);
    /// assert_eq!(recovery.recovery_delay(), Some(Duration::from_secs(30)));
    ///
    /// // Unavailability with recovery estimate
    /// let recovery = Recovery::unavailable().delay(Duration::from_secs(300));
    /// assert_eq!(recovery.kind(), RecoveryKind::Unavailable);
    /// assert_eq!(recovery.recovery_delay(), Some(Duration::from_secs(300)));
    /// ```
    #[must_use]
    pub const fn delay(self, delay: Duration) -> Self {
        Self {
            kind: self.kind,
            delay: Some(delay),
        }
    }

    /// Returns the recovery kind.
    ///
    /// Use this method to determine the appropriate recovery strategy
    /// for the given recovery instance.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{Recovery, RecoveryKind};
    ///
    /// let recovery = Recovery::unknown();
    /// assert_eq!(recovery.kind(), RecoveryKind::Unknown);
    ///
    /// let recovery = Recovery::retry();
    /// assert_eq!(recovery.kind(), RecoveryKind::Retry);
    /// ```
    #[must_use]
    pub const fn kind(&self) -> RecoveryKind {
        self.kind
    }

    /// Returns the explicit delay duration for recoverable conditions.
    ///
    /// Use this method with [`Recovery::kind`] to determine both whether a condition is recoverable
    /// and if an explicit delay is provided. This method returns `Some(duration)` when a delay
    /// has been specified via [`Recovery::delay`], and `None` otherwise.
    ///
    /// The meaning of the delay depends on the recovery kind:
    /// - For [`Recovery::retry`]: High-confidence timing guidance indicating when recovery will likely succeed.
    /// - For [`Recovery::unavailable`]: Low-confidence estimate for the earliest time when recovery might succeed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use recoverable::Recovery;
    ///
    /// // Specific delay requested with high confidence of success
    /// let delay = Recovery::retry().delay(Duration::from_secs(30));
    /// assert_eq!(delay.recovery_delay(), Some(Duration::from_secs(30)));
    ///
    /// // No delay specified
    /// let immediate = Recovery::retry();
    /// assert_eq!(immediate.recovery_delay(), None);
    ///
    /// // Unavailability with no recovery estimate
    /// let unavailable = Recovery::unavailable();
    /// assert_eq!(unavailable.recovery_delay(), None);
    ///
    /// // Unavailability with low-confidence recovery estimate
    /// let unavailable_with_time = Recovery::unavailable().delay(Duration::from_secs(300));
    /// assert_eq!(
    ///     unavailable_with_time.recovery_delay(),
    ///     Some(Duration::from_secs(300))
    /// );
    ///
    /// // Non-recoverable
    /// let never = Recovery::never();
    /// assert_eq!(never.recovery_delay(), None);
    /// ```
    #[must_use]
    pub const fn recovery_delay(&self) -> Option<Duration> {
        self.delay
    }
}

/// Enables types to indicate their recoverability metadata.
///
/// Implement this trait for errors or any type that can provide recovery metadata
/// information about its state. This allows consistent handling of recoverable
/// conditions across various types in resilience middlewares.
///
/// Typical scenarios for implementing this trait is an error that can be classified
/// as transient or permanent, depending on the specific error condition.
///
/// # Examples
///
/// Basic implementation for a simple error type:
///
/// ```rust
/// use recoverable::{Recover, Recovery};
///
/// #[derive(Debug)]
/// enum DatabaseError {
///     ConnectionTimeout,
///     InvalidCredentials,
///     TableNotFound,
/// }
///
/// impl Recover for DatabaseError {
///     fn recovery(&self) -> Recovery {
///         match self {
///             DatabaseError::ConnectionTimeout => Recovery::retry(),
///             DatabaseError::InvalidCredentials => Recovery::never(),
///             DatabaseError::TableNotFound => Recovery::never(),
///         }
///     }
/// }
/// ```
pub trait Recover {
    /// Returns the recovery metadata for this condition.
    ///
    /// Return appropriate recovery metadata based on the internal state of the type
    /// that implements this trait.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{Recover, Recovery, RecoveryKind};
    ///
    /// struct MyError;
    ///
    /// impl Recover for MyError {
    ///     fn recovery(&self) -> Recovery {
    ///         Recovery::retry()
    ///     }
    /// }
    ///
    /// let error = MyError;
    /// assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
    /// ```
    fn recovery(&self) -> Recovery;
}

impl Recover for Recovery {
    fn recovery(&self) -> Recovery {
        self.clone()
    }
}

impl<R, E> Recover for Result<R, E>
where
    R: Recover,
    E: Recover,
{
    fn recovery(&self) -> Recovery {
        match self {
            Ok(res) => res.recovery(),
            Err(err) => err.recovery(),
        }
    }
}

impl Display for Recovery {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.kind, f)
    }
}

impl Display for RecoveryKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => write!(f, "unknown"),
            Self::Never => write!(f, "never"),
            Self::Retry => write!(f, "retry"),
            Self::Unavailable => write!(f, "unavailable"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use static_assertions::{assert_impl_all, assert_not_impl_all};

    use super::*;

    #[test]
    fn assert_types() {
        assert_impl_all!(Recovery: Debug, PartialEq, Clone, Send, Sync);
        assert_impl_all!(RecoveryKind: Debug, PartialEq, Clone, Eq, Copy, std::hash::Hash);

        // cannot be Copy because in the future we may want to add more fields that are not Copy
        assert_not_impl_all!(Recovery: Copy);
    }

    #[test]
    fn recovery_enum() {
        assert_eq!(Recovery::unknown().kind(), RecoveryKind::Unknown);
        assert_eq!(Recovery::unavailable().kind(), RecoveryKind::Unavailable);
        assert_eq!(Recovery::retry().kind(), RecoveryKind::Retry);
        assert_eq!(Recovery::retry().delay(Duration::ZERO).kind(), RecoveryKind::Retry);
        assert_eq!(Recovery::never().kind(), RecoveryKind::Never);
    }

    #[test]
    fn display_ok() {
        assert_eq!(Recovery::unknown().to_string(), "unknown");
        assert_eq!(Recovery::never().to_string(), "never");
        assert_eq!(Recovery::retry().to_string(), "retry");
        assert_eq!(Recovery::unavailable().to_string(), "unavailable");
        assert_eq!(Recovery::retry().delay(Duration::from_secs(30)).to_string(), "retry");
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
        let recovery = Recovery::retry().delay(thirty_seconds);

        assert_eq!(recovery.recovery_delay(), Some(thirty_seconds));
        assert_eq!(recovery.kind(), RecoveryKind::Retry);

        // Zero duration
        let zero_duration = Recovery::retry().delay(Duration::ZERO);
        assert_eq!(zero_duration.recovery_delay(), Some(Duration::ZERO));

        // Delay can be applied to any recovery kind
        let unavailable = Recovery::unavailable().delay(Duration::from_secs(300));
        assert_eq!(unavailable.recovery_delay(), Some(Duration::from_secs(300)));
        assert_eq!(unavailable.kind(), RecoveryKind::Unavailable);

        // Applying delay multiple times replaces the previous delay
        let updated = Recovery::retry().delay(Duration::from_secs(10)).delay(Duration::from_secs(20));
        assert_eq!(updated.recovery_delay(), Some(Duration::from_secs(20)));
    }

    #[test]
    fn unavailable_behavior() {
        let recovery = Recovery::unavailable();
        assert_eq!(recovery.recovery_delay(), None);

        let recovery = Recovery::unavailable().delay(Duration::ZERO);
        assert_eq!(recovery.recovery_delay(), Some(Duration::ZERO));

        let recovery = Recovery::unavailable().delay(Duration::from_secs(1));
        assert_eq!(recovery.recovery_delay(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn assert_result_implements_recover() {
        #[derive(Debug)]
        pub struct RecoverableType(Recovery);

        impl Recover for RecoverableType {
            fn recovery(&self) -> Recovery {
                self.0.clone()
            }
        }

        assert_impl_all!(Result<RecoverableType, RecoverableType>: Recover);
        assert_not_impl_all!(Result<RecoverableType, String>: Recover);
    }

    #[test]
    fn recovery_delay_ok() {
        assert_eq!(Recovery::unknown().recovery_delay(), None);
        assert_eq!(Recovery::never().recovery_delay(), None);
        assert_eq!(Recovery::retry().recovery_delay(), None);
        assert_eq!(
            Recovery::retry().delay(Duration::from_secs(60)).recovery_delay(),
            Some(Duration::from_secs(60))
        );
        assert_eq!(Recovery::unavailable().recovery_delay(), None);
        assert_eq!(
            Recovery::unavailable().delay(Duration::from_secs(300)).recovery_delay(),
            Some(Duration::from_secs(300))
        );
    }

    #[test]
    fn recover_trait_implementations() {
        // Recovery implements Recover
        assert_eq!(Recovery::retry().recovery().kind(), RecoveryKind::Retry);

        assert_eq!(
            (Ok(TestType) as Result<TestType, TestType>).recovery().kind(),
            RecoveryKind::Unknown
        );
        assert_eq!(
            (Err(TestType) as Result<TestType, TestType>).recovery().kind(),
            RecoveryKind::Unknown
        );
    }

    // Result implements Recover
    #[derive(Debug)]
    struct TestType;
    impl Recover for TestType {
        fn recovery(&self) -> Recovery {
            Recovery::unknown()
        }
    }
}
