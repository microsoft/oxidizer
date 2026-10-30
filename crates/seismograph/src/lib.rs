// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Runtime telemetry for Rust applications.
//!
//! `seismograph` collects allocation, synchronization, I/O, cache, and task events
//! to help investigate memory use, contention, and scheduling. Crates such as
//! `rallocator` and `performables` provide instrumentation.
//!
//! Recording is off by default, with sampling and optional backtraces to control
//! its cost. Events are kept in bounded per-thread buffers. Snapshots combine
//! those events with state supplied by registered sources, such as allocators
//! and runtimes.
//!
//! # Usage
//!
//! Enable the event categories you need, run your workload, then capture a
//! snapshot. For ownership and synchronization events:
//!
//! ```rust
//! use seismograph::recorder::event::{EventClass, EventKind, ObjectId, Record};
//! use seismograph::recorder::{Configuration, Policy};
//!
//! seismograph::recorder(Configuration {
//!     general_events: Policy::all(false),
//!     ..Default::default()
//! });
//!
//! // Run application code that uses instrumented libraries.
//! // To emit an event directly:
//! seismograph::record(EventClass::General, || {
//!     Some(Record::object(EventKind::MutexAccess, ObjectId::new(1)))
//! });
//! let snapshot = seismograph::snapshot(Default::default())?;
//! # let _decoded = seismograph::snapshot::decode(snapshot.as_bytes())?;
//! # seismograph::recorder(Configuration::default());
//! # Ok::<(), seismograph::Error>(())
//! ```
//!
//! Inspect saved snapshots with `seismograph view` from the
//! [Seismograph CLI](https://crates.io/crates/seismograph_cli).
//!
//! # Live monitoring
//!
//! Enable `monitor`, call `seismograph::monitor::Monitor::builder().start()?`, and keep the returned handle alive.
//! Run `seismograph monitor` to connect.
//!
//! # Features
//!
//! - **`monitor`**: Enables the OS-authorized local monitor server.
//! - **`test-util`**: Enables `monitor`, the protocol's mock I/O controls, and
//!   `monitor::Builder::isolated` for native tests using a temporary registry.

use std::fmt;
use std::path::PathBuf;

/// OS-authorized local monitor server.
#[cfg(feature = "monitor")]
#[cfg_attr(docsrs, doc(cfg(feature = "monitor")))]
pub mod monitor;
pub mod recorder;
/// Snapshot types.
pub mod snapshot;
mod system;

/// Opaque error reported by Seismograph operations.
///
/// [`std::error::Error::source`] exposes the underlying cause when available.
pub struct Error {
    kind: ErrorKind,
}

enum ErrorKind {
    Message(&'static str),
    SourceFailed {
        source: snapshot::SourceId,
        error: Box<Error>,
    },
    DuplicateSource(snapshot::SourceId),
    InvalidFormat,
    WriteFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[cfg(feature = "monitor")]
    Monitor(monitor::StartupFailure),
}

impl Error {
    /// Creates an error with a static message.
    #[must_use]
    pub const fn new(message: &'static str) -> Self {
        Self {
            kind: ErrorKind::Message(message),
        }
    }

    pub(crate) const fn allocation_failed() -> Self {
        Self::new("seismograph snapshot storage could not be allocated")
    }

    pub(crate) const fn invalid_format() -> Self {
        Self {
            kind: ErrorKind::InvalidFormat,
        }
    }

    pub(crate) fn source_failed(source: snapshot::SourceId, error: Self) -> Self {
        Self {
            kind: ErrorKind::SourceFailed {
                source,
                error: Box::new(error),
            },
        }
    }

    pub(crate) const fn duplicate_source(source: snapshot::SourceId) -> Self {
        Self {
            kind: ErrorKind::DuplicateSource(source),
        }
    }

    pub(crate) fn write_file(path: PathBuf, source: std::io::Error) -> Self {
        Self {
            kind: ErrorKind::WriteFile { path, source },
        }
    }

    #[cfg(feature = "monitor")]
    fn monitor(failure: monitor::StartupFailure) -> Self {
        Self {
            kind: ErrorKind::Monitor(failure),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Error({self})")
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Message(message) => f.write_str(message),
            ErrorKind::SourceFailed { source, error } => {
                write!(f, "seismograph source {} failed: {error}", source.get())
            }
            ErrorKind::DuplicateSource(source) => {
                write!(f, "seismograph source {} was registered more than once", source.get())
            }
            ErrorKind::InvalidFormat => f.write_str("the seismograph snapshot is malformed or unsupported"),
            ErrorKind::WriteFile { path, source } => {
                write!(f, "failed to write seismograph snapshot to {}: {source}", path.display())
            }
            #[cfg(feature = "monitor")]
            ErrorKind::Monitor(failure) => fmt::Display::fmt(failure, f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::SourceFailed { error, .. } => Some(error),
            ErrorKind::WriteFile { source, .. } => Some(source),
            #[cfg(feature = "monitor")]
            ErrorKind::Monitor(failure) => std::error::Error::source(failure),
            _ => None,
        }
    }
}

/// Configures runtime event recording.
pub fn recorder(configuration: recorder::Configuration) {
    recorder::configure(configuration);
}

/// Builds and records an event when its category is enabled.
///
/// `build` runs only when `class` is enabled and this thread is not suppressed.
/// Return `None` from `build` to skip the event. The recorder applies sampling
/// and captures a backtrace according to the category and event policies.
/// Events from a different category or a concurrent configuration change are dropped.
///
/// # Panics
///
/// Propagates a panic from `build`.
pub fn record(class: recorder::event::EventClass, build: impl FnOnce() -> Option<recorder::event::Record>) {
    let _ = recorder::try_record(class, build);
}

/// Captures runtime events and all registered sources using `options`.
///
/// # Errors
///
/// Returns an error when capture or encoding fails.
pub fn snapshot(options: snapshot::SnapshotOptions) -> Result<snapshot::Snapshot, Error> {
    snapshot::snapshot(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_report_context_and_sources() {
        let source_id = snapshot::SourceId::new(7);
        let cases = [
            (Error::new("custom failure"), "custom failure".to_owned(), false),
            (
                Error::allocation_failed(),
                "seismograph snapshot storage could not be allocated".to_owned(),
                false,
            ),
            (
                Error::invalid_format(),
                "the seismograph snapshot is malformed or unsupported".to_owned(),
                false,
            ),
            (
                Error::source_failed(source_id, Error::new("source detail")),
                "seismograph source 7 failed: source detail".to_owned(),
                true,
            ),
            (
                Error::duplicate_source(source_id),
                "seismograph source 7 was registered more than once".to_owned(),
                false,
            ),
            (
                Error::write_file(PathBuf::from("snapshot.bin"), std::io::Error::other("disk full")),
                "failed to write seismograph snapshot to snapshot.bin: disk full".to_owned(),
                true,
            ),
        ];

        for (error, expected, has_source) in cases {
            assert_eq!(error.to_string(), expected);
            assert_eq!(format!("{error:?}"), format!("Error({expected})"));
            assert_eq!(std::error::Error::source(&error).is_some(), has_source);
        }
    }
}
