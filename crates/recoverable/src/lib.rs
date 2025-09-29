// Copyright (c) Microsoft Corporation.

//! Recovery metadata and classification for resilience patterns.
//!
//! This crate provides types for classifying error conditions as recoverable or non-recoverable,
//! enabling consistent retry behavior across different error types and resilience middleware.
//!
//! # Core Types
//!
//! - [`Recovery`]: Classifies errors as recoverable (transient) or non-recoverable (permanent).
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
//!             DatabaseError::ConnectionTimeout => Recovery::retry(),
//!             DatabaseError::InvalidCredentials => Recovery::never(),
//!             DatabaseError::TableNotFound => Recovery::never(),
//!         }
//!     }
//! }
//!
//! let error = DatabaseError::ConnectionTimeout;
//! assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
//! ```

use std::fmt::{Display, Formatter};
use std::time::Duration;

/// Classifies error conditions as temporary or permanent.
///
/// Use this type to determine whether retrying a failed operation is likely to succeed.
/// This helps implement appropriate retry strategies and avoid wasting resources on
/// unrecoverable conditions.
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
pub struct Recovery(RecoveryInner);

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

    /// The condition is temporary and may resolve quickly if retried.
    ///
    /// Use for transient failures that are expected to resolve relatively quickly,
    /// such as network timeouts, brief resource contention, or rate limiting.
    /// These conditions typically resolve within seconds to minutes.
    ///
    /// For service-wide outages that may take much longer to resolve,
    /// use [`Recovery::outage`] instead.
    Retry,

    /// The condition is permanent and retrying won't help.
    Never,

    /// Indicates a service-wide outage or significant degradation.
    ///
    /// Unlike `Retry`, outages represent widespread service failures that may take
    /// much longer to resolve and have uncertain recovery timelines. Retry strategies
    /// should use longer delays (minutes to hours) and expect multiple failures
    /// before recovery occurs.
    ///
    /// Some resilience middleware (such as circuit breakers) may choose to skip
    /// retry attempts entirely when an outage is detected, instead failing fast
    /// or routing to alternative services to avoid contributing to system load
    /// during the outage.
    Outage,
}

impl Recovery {
    /// Recovery cannot be determined.
    ///
    /// Use when it's unclear whether retrying would help. Consider treating
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
    pub const fn unknown() -> Self {
        Self(RecoveryInner::Unknown)
    }

    /// The condition is permanent and retrying won't help.
    ///
    /// Examples: malformed requests, or resource not found errors.
    /// These typically require user intervention or code changes to resolve.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use recoverable::{Recovery, RecoveryKind};
    ///
    /// let recovery = Recovery::never();
    /// assert_eq!(recovery.kind(), RecoveryKind::Never);
    /// assert_eq!(recovery.recovery_delay(), None);
    /// ```
    pub const fn never() -> Self {
        Self(RecoveryInner::Never)
    }

    /// The condition is temporary and may resolve quickly if retried.
    ///
    /// Use for transient failures that are expected to resolve relatively quickly,
    /// such as network timeouts, brief resource contention, or rate limiting.
    /// These conditions typically resolve within seconds to minutes without any
    /// specific timing guidance from the service.
    ///
    /// For service-wide outages that may take much longer to resolve,
    /// use [`Recovery::outage`] instead. For cases where the service provides
    /// explicit timing guidance, use [`Recovery::retry_after`].
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
    pub const fn retry() -> Self {
        Self(RecoveryInner::Retry)
    }

    /// The condition is temporary and may resolve after the specified duration.
    ///
    /// Use when the service explicitly indicates how long to wait before retrying,
    /// such as through HTTP 429 Rate Limited responses with a `Retry-After` header.
    /// This provides high-confidence timing guidance from the service about when
    /// the retry is likely to succeed.
    ///
    /// If the duration is zero, it's a hint to retry immediately.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use recoverable::{Recovery, RecoveryKind};
    ///
    /// // Service indicates to retry after 30 seconds
    /// let recovery = Recovery::retry_after(Duration::from_secs(30));
    /// assert_eq!(recovery.kind(), RecoveryKind::Retry);
    /// assert_eq!(recovery.recovery_delay(), Some(Duration::from_secs(30)));
    ///
    /// // Zero duration is equivalent to immediate retry
    /// let immediate = Recovery::retry_after(Duration::ZERO);
    /// assert_eq!(immediate.kind(), RecoveryKind::Retry);
    /// assert_eq!(immediate.recovery_delay(), Some(Duration::ZERO));
    /// ```
    pub const fn retry_after(duration: Duration) -> Self {
        Self(RecoveryInner::RetryAfter(duration))
    }

    /// Indicates a service is experiencing a widespread outage or significant degradation.
    ///
    /// Use when the failure is due to a service-wide outage that affects many users
    /// and may take an extended period to resolve (minutes to hours). Unlike
    /// [`Recovery::retry`] which suggests quick resolution, or [`Recovery::retry_after`]
    /// which provides high-confidence timing, outages indicate uncertainty about
    /// recovery timing and suggest that multiple retry attempts may fail before
    /// the service recovers.
    ///
    /// Retry strategies should implement exponential backoff with much longer delays
    /// than normal retries, as immediate recovery is unlikely and aggressive retrying
    /// may worsen the outage. Some recovery strategies may choose to skip retrying
    /// outages entirely or route to alternative services to avoid overloading the
    /// service that is experiencing the outage.
    ///
    /// # Parameters
    ///
    /// * `recovery_hint` - Optional hint for the earliest time when recovery attempts
    ///   might succeed. This represents a "do not retry before" threshold - attempts
    ///   before this time are expected to fail. If `None`, no recovery timeline
    ///   can be estimated.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use recoverable::Recovery;
    ///
    /// // Basic outage with no recovery hint
    /// let recovery = Recovery::outage(None);
    ///
    /// // Outage with low-confidence recovery estimate (chance it might recover in 5 minutes)
    /// let recovery = Recovery::outage(Some(Duration::from_secs(300)));
    /// ```
    pub const fn outage(recovery_hint: Option<Duration>) -> Self {
        Self(RecoveryInner::Outage(recovery_hint))
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
    pub fn kind(&self) -> RecoveryKind {
        match self.0 {
            RecoveryInner::Unknown => RecoveryKind::Unknown,
            RecoveryInner::Never => RecoveryKind::Never,
            RecoveryInner::Retry => RecoveryKind::Retry,
            RecoveryInner::RetryAfter(_) => RecoveryKind::Retry,
            RecoveryInner::Outage(_) => RecoveryKind::Outage,
        }
    }

    /// Returns the explicit delay duration for recoverable conditions.
    ///
    /// Use this method with [`Recovery::kind`] to determine both whether a condition is recoverable
    /// and if an explicit delay is provided. This method returns `Some(duration)` for
    /// recoverable conditions that specify a delay, and `None` for non-recoverable
    /// conditions or when no explicit delay is given.
    ///
    /// # Return Values
    ///
    /// - [`Recovery::retry_after`] returns the specified duration (can be `Duration::ZERO` for immediate retry)
    ///   This indicates a high-confidence expectation that retry will succeed after this duration.
    /// - [`Recovery::retry`] returns `None`
    /// - [`Recovery::outage`] returns the provided duration, or `None` if none was provided.
    ///   When present, this represents the earliest time when recovery attempts might succeed.
    ///   Attempts before this time are expected to fail.
    /// - [`Recovery::never`] and [`Recovery::unknown`] return `None`
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::time::Duration;
    ///
    /// use recoverable::Recovery;
    ///
    /// // Specific delay requested with high confidence of success
    /// let delay = Recovery::retry_after(Duration::from_secs(30));
    /// assert_eq!(delay.recovery_delay(), Some(Duration::from_secs(30)));
    ///
    /// // Immediate retry
    /// let immediate = Recovery::retry();
    /// assert_eq!(immediate.recovery_delay(), None);
    ///
    /// // Outage with no recovery estimate
    /// let outage = Recovery::outage(None);
    /// assert_eq!(outage.recovery_delay(), None);
    ///
    /// // Outage with low-confidence recovery estimate
    /// let outage_with_time = Recovery::outage(Some(Duration::from_secs(300)));
    /// assert_eq!(
    ///     outage_with_time.recovery_delay(),
    ///     Some(Duration::from_secs(300))
    /// );
    ///
    /// // Non-recoverable
    /// let never = Recovery::never();
    /// assert_eq!(never.recovery_delay(), None);
    /// ```
    pub fn recovery_delay(&self) -> Option<Duration> {
        match self.0 {
            RecoveryInner::RetryAfter(duration) => Some(duration),
            RecoveryInner::Retry => None,
            RecoveryInner::Outage(duration) => duration,
            _ => None,
        }
    }
}

/// Enables types to indicate their error conditions are recoverable.
///
/// Implement this trait for error types to provide a standardized way of communicating
/// whether retrying an operation might succeed. This enables consistent retry behavior
/// across different error types.
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
    /// Returns the Recovery classification for this error condition.
    ///
    /// Analyze the specific error condition and return the appropriate [`Recovery`]
    /// variant based on whether the condition is likely to resolve through retrying.
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

#[derive(Debug, PartialEq, Clone)]
enum RecoveryInner {
    Unknown,
    Never,
    Retry,
    RetryAfter(Duration),
    Outage(Option<Duration>),
}

impl Display for Recovery {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            RecoveryInner::Unknown => write!(f, "unknown"),
            RecoveryInner::Never => write!(f, "never"),
            RecoveryInner::Retry => write!(f, "retry"),
            RecoveryInner::RetryAfter(_) => write!(f, "retry-after"),
            RecoveryInner::Outage(_) => write!(f, "outage"),
        }
    }
}

impl Display for RecoveryKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RecoveryKind::Unknown => write!(f, "unknown"),
            RecoveryKind::Never => write!(f, "never"),
            RecoveryKind::Retry => write!(f, "retry"),
            RecoveryKind::Outage => write!(f, "outage"),
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

        // cannot be Copy because in the future we may want to add more fields that are not Copy
        assert_not_impl_all!(Recovery: Copy);
    }

    #[test]
    fn recovery_enum() {
        assert_eq!(Recovery::unknown().kind(), RecoveryKind::Unknown);
        assert_eq!(Recovery::outage(None).kind(), RecoveryKind::Outage);
        assert_eq!(Recovery::retry().kind(), RecoveryKind::Retry);
        assert_eq!(Recovery::retry_after(Duration::ZERO).kind(), RecoveryKind::Retry);
        assert_eq!(Recovery::never().kind(), RecoveryKind::Never);
    }

    #[test]
    fn display_ok() {
        assert_eq!(Recovery::unknown().to_string(), "unknown");
        assert_eq!(Recovery::never().to_string(), "never");
        assert_eq!(Recovery::retry().to_string(), "retry");
        assert_eq!(Recovery::outage(None).to_string(), "outage");
    }

    #[test]
    fn recovery_kind_display_ok() {
        assert_eq!(RecoveryKind::Unknown.to_string(), "unknown");
        assert_eq!(RecoveryKind::Never.to_string(), "never");
        assert_eq!(RecoveryKind::Retry.to_string(), "retry");
        assert_eq!(RecoveryKind::Outage.to_string(), "outage");
    }

    #[test]
    fn retry_after_behavior() {
        let thirty_seconds = Duration::from_secs(30);
        let recovery = Recovery::retry_after(thirty_seconds);

        assert_eq!(recovery.recovery_delay(), Some(thirty_seconds));

        // Zero duration should be equivalent to retry
        let zero_duration = Recovery::retry_after(Duration::ZERO);
        assert_eq!(zero_duration.recovery_delay(), Some(Duration::ZERO));
    }

    #[test]
    fn outage_behavior() {
        let recovery = Recovery::outage(None);
        assert_eq!(recovery.recovery_delay(), None);

        let recovery = Recovery::outage(Some(Duration::ZERO));
        assert_eq!(recovery.recovery_delay(), Some(Duration::ZERO));

        let recovery = Recovery::outage(Some(Duration::from_secs(1)));
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

        impl From<Recovery> for RecoverableType {
            fn from(recovery: Recovery) -> Self {
                Self(recovery)
            }
        }

        assert_impl_all!(Result<RecoverableType, RecoverableType>: Recover);
        assert_not_impl_all!(Result<RecoverableType, String>: Recover);
    }
}
