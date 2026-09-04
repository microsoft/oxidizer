// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! High-performance process telemetry with extensible snapshot sources.
//!
//! Instrumented crates record bounded events while registered sources contribute
//! point-in-time state to a portable [`snapshot()`].
//!
//! ```
//! use seismograph::recorder::event::{EventClass, EventKind, ObjectId, Record};
//! use seismograph::recorder::{Configuration, RecordingPolicy};
//!
//! seismograph::recorder(Configuration {
//!     arc_dereferences: RecordingPolicy::all(false),
//!     ..Default::default()
//! });
//! seismograph::record(EventClass::ArcDereference, || {
//!     Record::object(EventKind::ArcDeref, ObjectId::new(42))
//! });
//!
//! let encoded = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
//! let decoded = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();
//! assert!(
//!     decoded
//!         .events
//!         .events
//!         .iter()
//!         .any(|event| event.object_id() == Some(ObjectId::new(42)))
//! );
//! ```
//!
//! Applications built with the `monitor` feature can publish a localhost
//! endpoint for the `seismograph monitor` TUI. Keep the returned monitor alive
//! for as long as remote control should remain available:
//!
//! ```
//! # #[cfg(feature = "monitor")]
//! # {
//! let _monitor = seismograph::monitor::Monitor::builder()
//!     .name("worker")
//!     .instance("west-europe")
//!     .start()
//!     .unwrap();
//! # }
//! ```

use std::fmt;
use std::path::PathBuf;

/// Localhost monitor server.
#[cfg(feature = "monitor")]
pub mod monitor;
/// Event types.
pub mod recorder;
/// Snapshot types.
pub mod snapshot;
#[doc(hidden)]
pub mod system;

/// Opaque error reported by seismograph operations.
pub struct Error {
    kind: ErrorKind,
}

enum ErrorKind {
    Message(&'static str),
    SourceFailed { source: snapshot::SourceId, error: Box<Error> },
    DuplicateSource(snapshot::SourceId),
    InvalidFormat,
    WriteFile { path: PathBuf, source: std::io::Error },
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
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::SourceFailed { error, .. } => Some(error),
            ErrorKind::WriteFile { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Configures runtime event recording.
pub fn recorder(configuration: recorder::Configuration) {
    recorder::configure(configuration);
}

/// Lazily constructs and records an event in an independently configured class.
pub fn record(class: recorder::event::EventClass, event: impl FnOnce() -> recorder::event::Record) {
    recorder::record(class, event);
}

/// Records an event only while its originating session remains active.
#[doc(hidden)]
pub fn record_in_session(session: recorder::RecordingSession, event: impl FnOnce() -> recorder::event::Record) -> bool {
    recorder::record_in_session(session, event)
}

/// Records a classified event only while its originating session remains active.
#[doc(hidden)]
pub fn record_in_session_classified(
    session: recorder::RecordingSession,
    class: recorder::event::EventClass,
    event: impl FnOnce() -> recorder::event::Record,
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
    fn session_wrappers_record_only_in_the_selected_class() {
        let _test = recorder::TEST_LOCK.lock().unwrap();
        recorder(recorder::Configuration {
            general_events: recorder::RecordingPolicy::all(false),
            runtime_tasks: recorder::RecordingPolicy::all(false),
            ..Default::default()
        });
        let object_id = ObjectId::new(41);
        let general = recorder::select_object(object_id).unwrap();
        let runtime = recorder::select_object_for(recorder::event::EventClass::RuntimeTask, object_id).unwrap();

        record(recorder::event::EventClass::General, || {
            Record::object(EventKind::MutexAccess, object_id)
        });
        assert!(record_in_session(general, || { Record::object(EventKind::MutexAccess, object_id) }));
        assert!(record_in_session_classified(
            runtime,
            recorder::event::EventClass::RuntimeTask,
            || Record::runtime(
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
            )
        ));
        recorder(recorder::Configuration::default());
    }
}
