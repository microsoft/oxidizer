// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::arc::Arc;

/// A configuration, native publishing, or counter-processing failure.
///
/// Errors preserve their underlying source. Native failures expose the original
/// Windows status through [`windows_code`](Self::windows_code).
#[derive(Clone, Debug)]
pub struct Error {
    kind: ErrorKind,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.kind, f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Binding { source } => Some(source.as_ref()),
            ErrorKind::Read { source } => Some(source.as_ref()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub(crate) enum ErrorKind {
    #[error("invalid performance-counter configuration: {reason}")]
    Configuration { reason: String },
    #[error("invalid observed counter binding")]
    Binding {
        #[source]
        source: Arc<observed::processing::MetricBindingError>,
    },
    #[error("cannot read a bound observed counter")]
    Read {
        #[source]
        source: Arc<observed::processing::MetricReadError>,
    },
    #[error("native performance counters require 64-bit Windows")]
    UnsupportedPlatform,
    #[error("{operation} failed with Windows status {code}")]
    #[cfg_attr(
        not(all(windows, target_pointer_width = "64")),
        expect(dead_code, reason = "constructed only by native publishing")
    )]
    Windows { operation: &'static str, code: u32 },
    #[error("metric {counter_id} overflowed u64; the update was rejected")]
    Overflow { counter_id: u32 },
    #[error("metric {counter_id} would fall below zero; the update was rejected")]
    Underflow { counter_id: u32 },
    #[error("performance-counter collection failed with Windows status {code}; snapshots are frozen")]
    #[cfg_attr(
        not(all(windows, target_pointer_width = "64")),
        expect(dead_code, reason = "constructed only by native publishing")
    )]
    Collection { code: u32 },
    #[error("performance-counter state lock was poisoned")]
    Poisoned,
}

impl Error {
    pub(crate) fn configuration(reason: impl Into<String>) -> Self {
        ErrorKind::Configuration { reason: reason.into() }.into()
    }

    /// Whether this error reports an unsupported operating system or pointer width.
    #[must_use]
    pub fn is_unsupported_platform(&self) -> bool {
        matches!(self.kind, ErrorKind::UnsupportedPlatform)
    }

    /// Whether startup rejected the configuration or its typed event bindings.
    #[must_use]
    pub fn is_invalid_configuration(&self) -> bool {
        matches!(self.kind, ErrorKind::Configuration { .. } | ErrorKind::Binding { .. })
    }

    /// The original Windows status, for a native or collection failure.
    #[must_use]
    pub fn windows_code(&self) -> Option<u32> {
        match self.kind {
            ErrorKind::Windows { code, .. } | ErrorKind::Collection { code } => Some(code),
            _ => None,
        }
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self { kind }
    }
}
