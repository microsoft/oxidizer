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
//! use seismograph::recorder::{Configuration, RecordingPolicy};
//!
//! seismograph::recorder(Configuration {
//!     general_events: RecordingPolicy::all(false),
//!     ..Default::default()
//! });
//!
//! // Run application code that uses instrumented libraries.
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
//! Applications built with the `monitor` feature can publish an OS-authorized local
//! endpoint for the `seismograph monitor` TUI. Keep the returned monitor alive
//! for as long as local control should remain available. The owning user and
//! root/elevated administrators can connect without a token. Startup creates
//! the user's private registry automatically; no administrator setup is needed.
//! Unix prefers `$XDG_RUNTIME_DIR/seismograph`, with a private randomized `/tmp`
//! fallback. Windows uses the user's local application-data directory.
//! Pass a `seismograph_protocol::io::MonitorIo` to `Builder::io` to select
//! the monitor's identity, transport, discovery, persistence, and clock
//! capability. Omitting it preserves native I/O; `seismograph_protocol`'s
//! `test-util` feature provides an isolated in-memory implementation for
//! application tests without host registry setup.
//!
//! ```no_run
//! # #[cfg(feature = "monitor")]
//! # {
//! let _monitor = seismograph::monitor::Monitor::builder()
//!     .name("worker")
//!     .instance("west-europe")
//!     .start()
//!     .unwrap();
//! # }
//! ```
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
/// Event types.
pub mod recorder;
/// Snapshot types.
pub mod snapshot;
#[doc(hidden)]
pub mod system;

/// Opaque error reported by snapshot operations or monitor startup.
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
    /// Creates an error for a snapshot source failure.
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

/// Lazily constructs and records an event in an independently configured class.
///
/// The closure runs only while recording for this class is active and not
/// suppressed. Return `None` to skip the event conditionally. Object sampling
/// is checked after the closure constructs a record.
pub fn record(class: recorder::event::EventClass, event: impl FnOnce() -> Option<recorder::event::Record>) {
    recorder::record(class, event);
}

/// Lazily constructs an event and returns the session that accepted it.
///
/// Returns `None` when the closure skips the event or no event is accepted.
#[doc(hidden)]
pub fn record_session(
    class: recorder::event::EventClass,
    event: impl FnOnce() -> Option<recorder::event::Record>,
) -> Option<recorder::RecordingSession> {
    recorder::record_session(class, event)
}

/// Records an event only while its originating session remains active.
///
/// Returns `false` when the closure skips the event or no event is accepted.
#[doc(hidden)]
pub fn record_in_session(session: recorder::RecordingSession, event: impl FnOnce() -> Option<recorder::event::Record>) -> bool {
    recorder::record_in_session(session, event)
}

/// Records a classified event only while its originating session remains active.
///
/// Returns `false` when the closure skips the event or no event is accepted.
#[doc(hidden)]
pub fn record_in_session_classified(
    session: recorder::RecordingSession,
    class: recorder::event::EventClass,
    event: impl FnOnce() -> Option<recorder::event::Record>,
) -> bool {
    recorder::record_in_session_classified(session, class, event)
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::recorder::event::{EventKind, ObjectId, Record};

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

    #[test]
    #[cfg_attr(coverage_nightly, coverage(off))] // Disabled sessions deliberately must not construct records.
    fn session_wrappers_record_only_in_the_selected_class() {
        let _test = recorder::TEST_LOCK.lock().unwrap();
        recorder(recorder::Configuration {
            general_events: recorder::RecordingPolicy::all(false),
            runtime_tasks: recorder::RecordingPolicy::all(false),
            ..recorder::test_configuration()
        });
        let object_id = ObjectId::new(41);
        let general = record_session(recorder::event::EventClass::General, || {
            Some(Record::object(EventKind::MutexAccess, object_id))
        })
        .unwrap();
        let runtime = recorder::select_object_for(recorder::event::EventClass::RuntimeTask, object_id).unwrap();

        assert!(record_in_session(general, || {
            Some(Record::object(EventKind::MutexAccess, object_id))
        }));
        assert!(record_in_session_classified(
            runtime,
            recorder::event::EventClass::RuntimeTask,
            || Some(Record::runtime(
                recorder::event::EventTimestamp::now(),
                EventKind::RuntimeCreated,
                recorder::runtime::RuntimeEvent {
                    runtime_id: recorder::runtime::RuntimeId::from_raw(1).unwrap(),
                    worker_id: None,
                    subject_id: 0,
                    related_id: 0,
                    value_0: 0,
                    value_1: 0,
                },
                recorder::event::BacktraceCapture::Never,
            ))
        ));
        recorder(recorder::Configuration::default());

        let constructed = AtomicUsize::new(0);
        assert!(!record_in_session(general, || {
            constructed.fetch_add(1, Ordering::Relaxed);
            Some(Record::object(EventKind::MutexAccess, object_id))
        }));
        assert!(!record_in_session_classified(
            runtime,
            recorder::event::EventClass::RuntimeTask,
            || {
                constructed.fetch_add(1, Ordering::Relaxed);
                Some(Record::runtime(
                    recorder::event::EventTimestamp::now(),
                    EventKind::RuntimeCreated,
                    recorder::runtime::RuntimeEvent {
                        runtime_id: recorder::runtime::RuntimeId::from_raw(1).unwrap(),
                        worker_id: None,
                        subject_id: 0,
                        related_id: 0,
                        value_0: 0,
                        value_1: 0,
                    },
                    recorder::event::BacktraceCapture::Never,
                ))
            }
        ));
        assert_eq!(constructed.load(Ordering::Relaxed), 0);
    }
}
