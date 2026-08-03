// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::backtrace::{Backtrace, BacktraceStatus};
use std::fmt;
use std::time::SystemTimeError;

/// The result type for fallible operations in this crate.
///
/// # Examples
///
/// ```
/// fn operation() -> tick::Result<()> {
///     Ok(())
/// }
///
/// operation()?;
/// # Ok::<(), tick::Error>(())
/// ```
pub type Result<T> = std::result::Result<T, Error>;

/// An error that can occur in the `tick` crate.
///
/// The most common type of error results from overflow, but other errors
/// also exist:
///
/// * Parsing and formatting errors.
/// * Validation problems.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "fmt")]
/// # {
/// use tick::Error;
/// use tick::fmt::Iso8601;
///
/// let result: Result<Iso8601, Error> = "invalid date".parse();
/// assert!(matches!(result, Err(error) if !error.is_timeout()));
/// # }
/// ```
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    backtrace: MaybeBacktrace,
}

crate::thread_aware_move!(Error);

#[derive(Debug)]
enum MaybeBacktrace {
    Captured(Box<Backtrace>),
    Disabled,
}

impl MaybeBacktrace {
    fn capture() -> Self {
        let backtrace = Backtrace::capture();

        match backtrace.status() {
            BacktraceStatus::Captured => Self::Captured(Box::new(backtrace)),
            _ => Self::Disabled,
        }
    }

    const fn get(&self) -> Option<&Backtrace> {
        match self {
            Self::Captured(backtrace) => Some(backtrace),
            Self::Disabled => None,
        }
    }
}

#[derive(Debug)]
enum ErrorKind {
    #[cfg(any(feature = "fmt", test))]
    Jiff(jiff::Error),
    #[cfg(any(feature = "fmt", test))]
    OutOfRange(std::borrow::Cow<'static, str>),
    Other(Box<dyn std::error::Error + Send + Sync + 'static>),
    SystemTimeError(SystemTimeError),
    Timeout,
}

impl Error {
    fn from_kind(kind: ErrorKind) -> Self {
        Self {
            kind,
            backtrace: MaybeBacktrace::capture(),
        }
    }

    #[cfg(any(feature = "fmt", test))]
    pub(super) fn out_of_range(message: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::from_kind(ErrorKind::OutOfRange(message.into()))
    }

    #[cfg(any(feature = "fmt", test))]
    pub(super) fn jiff(error: jiff::Error) -> Self {
        Self::from_kind(ErrorKind::Jiff(error))
    }

    pub(super) fn other(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::from_kind(ErrorKind::Other(Box::new(error)))
    }

    pub(super) fn timeout() -> Self {
        Self::from_kind(ErrorKind::Timeout)
    }

    /// Returns whether this error reports a future timeout.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "test-util")]
    /// # {
    /// use std::time::Duration;
    ///
    /// use tick::{ClockControl, FutureExt};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let clock = ClockControl::new_auto_advancing().to_clock();
    /// let error = clock
    ///     .delay(Duration::from_secs(2))
    ///     .timeout(&clock, Duration::from_secs(1))
    ///     .await
    ///     .err()
    ///     .ok_or_else(|| std::io::Error::other("future unexpectedly completed"))?;
    ///
    /// assert!(error.is_timeout());
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        matches!(&self.kind, ErrorKind::Timeout)
    }

    /// Returns whether this error reports a value outside a supported range.
    #[must_use]
    pub const fn is_out_of_range(&self) -> bool {
        #[cfg(any(feature = "fmt", test))]
        {
            matches!(&self.kind, ErrorKind::OutOfRange(_))
        }

        #[cfg(not(any(feature = "fmt", test)))]
        {
            false
        }
    }

    /// Returns the captured backtrace, when backtrace capture was enabled.
    #[must_use]
    pub const fn backtrace(&self) -> Option<&Backtrace> {
        self.backtrace.get()
    }

    #[cfg(test)]
    const fn kind(&self) -> &ErrorKind {
        &self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::Jiff(err) => err.fmt(f),
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::OutOfRange(msg) => write!(f, "{msg}"),
            ErrorKind::Other(err) => err.fmt(f),
            ErrorKind::SystemTimeError(err) => err.fmt(f),
            ErrorKind::Timeout => f.write_str("future timed out"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::Jiff(err) => Some(err),
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::OutOfRange(_) => None,
            ErrorKind::Other(err) => Some(err.as_ref()),
            ErrorKind::SystemTimeError(err) => Some(err),
            ErrorKind::Timeout => None,
        }
    }
}

impl From<SystemTimeError> for Error {
    fn from(err: SystemTimeError) -> Self {
        Self::from_kind(ErrorKind::SystemTimeError(err))
    }
}

impl From<std::num::ParseIntError> for Error {
    fn from(error: std::num::ParseIntError) -> Self {
        Self::other(error)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::time::{Duration, UNIX_EPOCH};

    use jiff::SignedDuration;
    use thread_aware::ThreadAware;
    use thread_aware::affinity::pinned_affinities;

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
            "parameter 'Unix timestamp seconds' is not in the required range of -377705023201..=253402207200"
        );
        assert!(error.source().is_some());
    }

    #[test]
    fn out_of_range_error() {
        let error = Error::out_of_range("test");

        assert!(matches!(error.kind(), ErrorKind::OutOfRange(_)));
        assert_eq!(error.to_string(), "test");
        assert!(error.source().is_none());
    }

    #[test]
    fn from_other_ok() {
        let error = Error::other(std::io::Error::other("dummy"));

        assert!(matches!(error.kind(), ErrorKind::Other(_)));
        assert_eq!(error.to_string(), "dummy");
        assert_eq!(error.source().unwrap().to_string(), "dummy");
    }

    #[test]
    fn from_system_time_error() {
        let later = UNIX_EPOCH + Duration::from_secs(1);
        let system_time_error = UNIX_EPOCH.duration_since(later).unwrap_err();
        let expected_message = system_time_error.to_string();

        let error = Error::from(system_time_error);

        assert!(matches!(error.kind(), ErrorKind::SystemTimeError(_)));
        assert_eq!(error.to_string(), expected_message);
        assert!(error.source().is_some());
    }

    #[test]
    fn timeout_error_is_classified() {
        let error = Error::timeout();

        assert!(error.is_timeout());
        assert_eq!(error.to_string(), "future timed out");
        assert!(error.source().is_none());
    }

    #[test]
    fn thread_aware_ok() {
        let error = Error::other(std::io::Error::other("dummy"));
        let affinities = pinned_affinities(&[2]);

        let mut error = error;
        error.relocate(Some(affinities[0]), affinities[0]);

        assert!(matches!(error.kind(), ErrorKind::Other(_)));
    }
}
