// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::backtrace::{Backtrace, BacktraceStatus};
use std::fmt;
use std::time::SystemTimeError;

/// An error from the `tick` crate.
///
/// Errors report clock lifecycle failures, timeouts, or failed conversions from
/// [`SystemTime`](std::time::SystemTime).
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
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    backtrace: Backtrace,
}

crate::thread_aware_move!(Error);

#[derive(Debug)]
enum ErrorKind {
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

    /// Returns the error backtrace.
    ///
    /// The status is [`BacktraceStatus::Disabled`](std::backtrace::BacktraceStatus::Disabled)
    /// when backtrace capture is disabled.
    pub const fn backtrace(&self) -> &Backtrace {
        &self.backtrace
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
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
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "native tests cover standard-library backtrace capture and writer errors")]
    fn gamma_round2_backtrace_formatting_preserves_a_writer_error() {
        use std::fmt::Write as _;

        struct FirstWriteOnly(bool);

        impl std::fmt::Write for FirstWriteOnly {
            fn write_str(&mut self, _: &str) -> std::fmt::Result {
                if std::mem::replace(&mut self.0, false) {
                    Ok(())
                } else {
                    Err(std::fmt::Error)
                }
            }
        }

        let error = super::Error {
            kind: super::ErrorKind::Timeout(std::io::Error::other("round2")),
            backtrace: std::backtrace::Backtrace::force_capture(),
        };
        assert!(write!(FirstWriteOnly(true), "{error}").is_err());
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
