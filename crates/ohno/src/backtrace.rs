use std::backtrace::{Backtrace as StdBacktrace, BacktraceStatus};
use std::sync::Arc;

/// A lightweight, clonable wrapper around [`std::backtrace::Backtrace`].
///
/// This type provides cloning support for backtraces while minimizing overhead:
/// - Captured backtraces are stored in an `Arc` for efficient sharing
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
        match bt.status() {
            BacktraceStatus::Disabled => Self::Disabled,
            BacktraceStatus::Unsupported => Self::Unsupported,
            _ => Self::Captured(Arc::new(bt)),
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
            Backtrace::Captured(bt) => bt.status(),
            Backtrace::Disabled => BacktraceStatus::Disabled,
            Backtrace::Unsupported => BacktraceStatus::Unsupported,
        }
    }

    /// Get a reference to the inner backtrace, if captured.
    pub(crate) fn as_backtrace(&self) -> &StdBacktrace {
        static DISABLED_BACKTRACE: StdBacktrace = StdBacktrace::disabled();
        match self {
            Backtrace::Captured(bt) => bt.as_ref(),
            _ => &DISABLED_BACKTRACE,
        }
    }
}
