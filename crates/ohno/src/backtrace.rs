// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A lightweight, clonable wrapper around [`Backtrace`].

use std::backtrace::{Backtrace as StdBacktrace, BacktraceStatus};
use std::sync::Arc;

/// A lightweight, clonable wrapper around [`Backtrace`].
///
/// This type provides cloning support for backtraces while minimizing overhead:
/// - Captured backtraces are stored in an [`Arc`] for efficient sharing
/// - Disabled and unsupported backtraces require no heap allocation
#[derive(Debug, Clone)]
pub(crate) enum Backtrace {
    /// A captured backtrace
    Captured(Arc<StdBacktrace>),
    /// A disabled backtrace
    Disabled,
    /// An unsupported backtrace
    Unsupported,
}

impl Backtrace {
    /// Create a `Backtrace` from a standard backtrace.
    pub(crate) fn from_backtrace(bt: StdBacktrace) -> Self {
        Self::from_status(&bt.status(), || Arc::new(bt))
    }

    fn from_status(status: &BacktraceStatus, captured: impl FnOnce() -> Arc<StdBacktrace>) -> Self {
        match status {
            BacktraceStatus::Disabled => Self::Disabled,
            BacktraceStatus::Unsupported => Self::Unsupported,
            _ => Self::Captured(captured()),
        }
    }

    /// Capture a new backtrace.
    pub(crate) fn capture() -> Self {
        let bt = StdBacktrace::capture();
        Self::from_backtrace(bt)
    }

    /// Force capture a new backtrace.
    #[cfg(test)]
    pub(crate) fn force_capture() -> Self {
        let bt = StdBacktrace::force_capture();
        Self::from_backtrace(bt)
    }

    /// Create a disabled backtrace.
    pub(crate) fn disabled() -> Self {
        Self::Disabled
    }

    /// Get the status of the backtrace.
    pub(crate) fn status(&self) -> BacktraceStatus {
        match self {
            Self::Captured(bt) => bt.status(),
            Self::Disabled => BacktraceStatus::Disabled,
            Self::Unsupported => BacktraceStatus::Unsupported,
        }
    }

    /// Get a reference to the inner backtrace, if captured.
    pub(crate) fn as_backtrace(&self) -> &StdBacktrace {
        static DISABLED_BACKTRACE: StdBacktrace = StdBacktrace::disabled();
        match self {
            Self::Captured(bt) => bt.as_ref(),
            _ => &DISABLED_BACKTRACE,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nov6_status_conversion_keeps_unsupported_distinct_and_captures_only_on_demand() {
        for (status, unsupported) in [(BacktraceStatus::Disabled, false), (BacktraceStatus::Unsupported, true)] {
            let converted = Backtrace::from_status(&status, || panic!("this status must not allocate a captured backtrace"));
            assert_eq!(matches!(converted, Backtrace::Unsupported), unsupported);
            assert_eq!(matches!(converted, Backtrace::Disabled), !unsupported);
        }
        let captured = Arc::new(StdBacktrace::disabled());
        let converted = Backtrace::from_status(&BacktraceStatus::Captured, || Arc::clone(&captured));
        let Backtrace::Captured(actual) = converted else {
            panic!("captured status must preserve the backtrace");
        };
        assert!(Arc::ptr_eq(&actual, &captured));
    }

    #[test]
    fn cloning() {
        let bt1 = Backtrace::capture();
        let bt2 = bt1.clone();
        assert_eq!(bt1.status(), bt2.status());
    }

    #[test]
    fn from_std_backtrace() {
        let std_bt = StdBacktrace::capture();
        let status = std_bt.status();
        let bt = Backtrace::from_backtrace(std_bt);
        assert_eq!(bt.status(), status);

        let disabled_bt = StdBacktrace::disabled();
        let bt_disabled = Backtrace::from_backtrace(disabled_bt);
        assert_eq!(bt_disabled.status(), BacktraceStatus::Disabled);
        assert!(matches!(bt_disabled, Backtrace::Disabled));

        let forced = StdBacktrace::force_capture();
        let status = forced.status();
        assert_eq!(Backtrace::from_backtrace(forced).status(), status);
    }

    #[test]
    fn status_conversion() {
        let bt = Backtrace::Captured(Arc::new(StdBacktrace::disabled()));
        assert_eq!(bt.status(), BacktraceStatus::Disabled);
        let bt = Backtrace::Captured(Arc::new(StdBacktrace::force_capture()));
        assert_eq!(bt.status(), BacktraceStatus::Captured);

        let bt = Backtrace::Disabled;
        assert_eq!(bt.status(), BacktraceStatus::Disabled);

        let bt = Backtrace::Unsupported;
        assert_eq!(bt.status(), BacktraceStatus::Unsupported);
    }
}
