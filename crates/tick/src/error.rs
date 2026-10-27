// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::backtrace::{Backtrace, BacktraceStatus};
use std::fmt;
use std::time::SystemTimeError;

/// An error from the `tick` crate.
///
/// Overflow is the most common cause. Other causes include:
///
/// - Parsing or formatting errors.
/// - Invalid values.
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
    backtrace: Backtrace,
}

crate::thread_aware_move!(Error);

#[derive(Debug)]
enum ErrorKind {
    #[cfg(any(feature = "fmt", test))]
    Jiff(jiff::Error),
    #[cfg(any(feature = "fmt", test))]
    OutOfRange {
        message: std::borrow::Cow<'static, str>,
        source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
    },
    #[cfg(any(feature = "fmt", test))]
    Other(Box<dyn std::error::Error + Send + Sync + 'static>),
    SystemTimeError(SystemTimeError),
    Timeout(std::io::Error),
}

impl Error {
    fn from_kind(kind: ErrorKind) -> Self {
        Self {
            kind,
            backtrace: Backtrace::capture(),
        }
    }

    #[cfg(any(feature = "fmt", test))]
    pub(super) fn out_of_range(message: impl Into<std::borrow::Cow<'static, str>>) -> Self {
        Self::from_kind(ErrorKind::OutOfRange {
            message: message.into(),
            source: None,
        })
    }

    #[cfg(any(feature = "fmt", test))]
    pub(super) fn out_of_range_from(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::from_kind(ErrorKind::OutOfRange {
            message: error.to_string().into(),
            source: Some(Box::new(error)),
        })
    }

    #[cfg(any(feature = "fmt", test))]
    pub(super) fn jiff(error: jiff::Error) -> Self {
        if error.is_range() {
            return Self::out_of_range_from(error);
        }
        Self::from_kind(ErrorKind::Jiff(error))
    }

    #[cfg(any(feature = "fmt", test))]
    pub(super) fn other(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::from_kind(ErrorKind::Other(Box::new(error)))
    }

    pub(super) fn timeout() -> Self {
        Self::from_kind(ErrorKind::Timeout(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "future timed out",
        )))
    }

    /// Returns whether this error reports a future timeout.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "test-util")]
    /// # {
    /// use std::future;
    /// use std::time::Duration;
    ///
    /// use tick::{ClockControl, FutureExt};
    ///
    /// # futures::executor::block_on(async {
    /// let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    /// let error = future::pending::<()>()
    ///     .timeout(&clock, Duration::from_secs(1))
    ///     .await
    ///     .unwrap_err();
    ///
    /// assert!(error.is_timeout());
    /// # });
    /// # }
    /// ```
    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        matches!(&self.kind, ErrorKind::Timeout(_))
    }

    /// Returns whether this error reports a value outside a supported range.
    ///
    /// An application can use this classification to distinguish an unsupported
    /// timestamp from malformed input without matching the error message.
    #[must_use]
    pub const fn is_out_of_range(&self) -> bool {
        #[cfg(any(feature = "fmt", test))]
        {
            matches!(&self.kind, ErrorKind::OutOfRange { .. })
        }

        #[cfg(not(any(feature = "fmt", test)))]
        {
            false
        }
    }

    /// Returns the error backtrace.
    ///
    /// The status is [`BacktraceStatus::Disabled`](std::backtrace::BacktraceStatus::Disabled)
    /// when backtrace capture is disabled.
    pub const fn backtrace(&self) -> &Backtrace {
        &self.backtrace
    }

    #[cfg(test)]
    const fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    #[cfg(test)]
    pub(crate) fn with_disabled_backtrace(mut self) -> Self {
        self.backtrace = Backtrace::disabled();
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::Jiff(err) => err.fmt(f),
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::OutOfRange { message, .. } => write!(f, "{message}"),
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::Other(err) => err.fmt(f),
            ErrorKind::SystemTimeError(err) => err.fmt(f),
            ErrorKind::Timeout(err) => err.fmt(f),
        }?;

        if self.backtrace.status() == BacktraceStatus::Captured {
            write!(f, "\n\nBacktrace:\n{}", self.backtrace)?;
        }

        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::Jiff(err) => Some(err),
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::OutOfRange { source, .. } => source.as_deref().map(|error| error as _),
            #[cfg(any(feature = "fmt", test))]
            ErrorKind::Other(err) => Some(err.as_ref()),
            ErrorKind::SystemTimeError(err) => Some(err),
            ErrorKind::Timeout(err) => Some(err),
        }
    }
}

impl From<SystemTimeError> for Error {
    fn from(err: SystemTimeError) -> Self {
        Self::from_kind(ErrorKind::SystemTimeError(err))
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn private_jiff_variant_preserves_message_and_source() {
        // Public format parsing covers construction; variant identity is intentionally private.
        let error = "date".parse::<jiff::Timestamp>().unwrap_err();
        let expected = error.to_string();
        let error = Error::jiff(error).with_disabled_backtrace();

        assert!(matches!(error.kind(), ErrorKind::Jiff(_)));
        assert_eq!(error.to_string(), expected);
        assert!(error.source().is_some());
    }

    #[test]
    fn private_out_of_range_variant_has_no_source() {
        // The public error type deliberately exposes no variant introspection.
        let error = Error::out_of_range("test").with_disabled_backtrace();

        assert!(matches!(error.kind(), ErrorKind::OutOfRange { .. }));
        assert!(error.is_out_of_range());
        assert!(!error.is_timeout());
        assert_eq!(error.to_string(), "test");
        assert!(error.source().is_none());
    }

    #[test]
    fn private_other_variant_preserves_message_and_source() {
        // No public constructor accepts an arbitrary boxed source.
        let error = Error::other(std::io::Error::other("dummy")).with_disabled_backtrace();

        assert!(matches!(error.kind(), ErrorKind::Other(_)));
        assert_eq!(error.to_string(), "dummy");
        assert_eq!(error.source().unwrap().to_string(), "dummy");
    }

    #[test]
    #[cfg_attr(miri, ignore = "native coverage exercises standard-library backtrace formatting")]
    fn captured_backtrace_is_available_from_the_error() {
        let backtrace = Backtrace::force_capture();
        assert_eq!(backtrace.status(), BacktraceStatus::Captured);

        let error = Error {
            kind: ErrorKind::Timeout(std::io::Error::new(std::io::ErrorKind::TimedOut, "future timed out")),
            backtrace,
        };

        assert_eq!(error.backtrace().status(), BacktraceStatus::Captured);
        let display: &dyn fmt::Display = &error;
        assert!(display.to_string().contains("\n\nBacktrace:\n"));
    }

    #[test]
    fn disabled_backtrace_is_available_from_the_error() {
        let backtrace = Backtrace::disabled();
        assert_eq!(backtrace.status(), BacktraceStatus::Disabled);

        let error = Error {
            kind: ErrorKind::Timeout(std::io::Error::new(std::io::ErrorKind::TimedOut, "future timed out")),
            backtrace,
        };

        assert_eq!(error.backtrace().status(), BacktraceStatus::Disabled);
    }
}
