// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Low-overhead I/O event instrumentation for [`seismograph`].
//!
//! [`Resource`] lazily acquires its identity when an enabled I/O event is first
//! recorded. [`Operation`] pairs start and finish events without reading a
//! clock or allocating any identity outside [`seismograph::record`].

use std::sync::atomic::{AtomicU64, Ordering};

use seismograph::recorder::event::{EventClass, EventKind, Record};
pub use seismograph::recorder::io::{BufferId, IoOutcome, IoResourceKind};
use seismograph::recorder::io::{IoEvent, IoOperationId, IoResourceId};

/// An I/O resource whose identity is allocated only when recording is enabled.
#[derive(Debug)]
pub struct Resource {
    id: AtomicU64,
    kind: IoResourceKind,
}

impl Resource {
    /// Creates an unrecorded I/O resource.
    #[must_use]
    pub const fn new(kind: IoResourceKind) -> Self {
        Self {
            id: AtomicU64::new(0),
            kind,
        }
    }

    /// Returns the resource kind.
    #[must_use]
    pub const fn kind(&self) -> IoResourceKind {
        self.kind
    }

    fn id(&self) -> IoResourceId {
        let current = self.id.load(Ordering::Relaxed);
        if let Some(id) = IoResourceId::from_raw(current) {
            return id;
        }

        let allocated = IoResourceId::allocate();
        match self.id.compare_exchange(0, allocated.get(), Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => allocated,
            Err(existing) => IoResourceId::from_raw(existing).expect("resource identity can only transition from zero to a valid ID"),
        }
    }
}

/// Buffer metadata captured for an I/O event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferState {
    id: Option<BufferId>,
    len: u64,
    span_count: u32,
}

impl BufferState {
    /// Captures metadata for an identified logical buffer.
    #[must_use]
    pub fn new(id: Option<BufferId>, len: usize, span_count: usize) -> Self {
        Self {
            id,
            len: u64::try_from(len).unwrap_or(u64::MAX),
            span_count: u32::try_from(span_count).unwrap_or(u32::MAX),
        }
    }

    /// Describes an operation without an associated logical buffer.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            id: None,
            len: 0,
            span_count: 0,
        }
    }

    /// Returns the logical buffer identity, when one is available.
    #[must_use]
    pub const fn id(self) -> Option<BufferId> {
        self.id
    }

    /// Returns the observed buffer length.
    #[must_use]
    pub const fn len(self) -> u64 {
        self.len
    }

    /// Returns whether the observed buffer was empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Returns the observed number of spans.
    #[must_use]
    pub const fn span_count(self) -> u32 {
        self.span_count
    }
}

/// Supplies lazily captured metadata for a subsystem-specific buffer type.
///
/// Implementations are called only from an enabled [`seismograph::record`] closure.
pub trait Buffer {
    /// Returns the current logical identity and shape of this buffer.
    fn recording_state(&self) -> BufferState;
}

/// A paired I/O operation whose start event may have been recorded.
#[derive(Debug)]
#[must_use = "finish or explicitly discard the I/O operation"]
pub struct Operation {
    recorded: Option<RecordedOperation>,
}

#[derive(Clone, Copy, Debug)]
struct RecordedOperation {
    operation_id: IoOperationId,
    resource_id: IoResourceId,
    buffer_id: Option<BufferId>,
    requested_bytes: u64,
    buffer_len: u64,
    buffer_span_count: u32,
    resource_kind: IoResourceKind,
    finish_kind: EventKind,
}

impl Operation {
    /// Records the start of a read operation when I/O recording is enabled.
    pub fn read_started(resource: &Resource, requested_bytes: u64, buffer: impl FnOnce() -> BufferState) -> Self {
        Self::started(
            resource,
            requested_bytes,
            buffer,
            EventKind::IoReadStarted,
            EventKind::IoReadFinished,
        )
    }

    /// Records the start of a write operation when I/O recording is enabled.
    pub fn write_started(resource: &Resource, requested_bytes: u64, buffer: impl FnOnce() -> BufferState) -> Self {
        Self::started(
            resource,
            requested_bytes,
            buffer,
            EventKind::IoWriteStarted,
            EventKind::IoWriteFinished,
        )
    }

    /// Returns whether the start event was constructed for an active recording session.
    #[must_use]
    pub const fn was_recorded(&self) -> bool {
        self.recorded.is_some()
    }

    /// Records completion using current buffer metadata.
    pub fn finish(self, completed_bytes: u64, outcome: IoOutcome, buffer: impl FnOnce() -> BufferState) {
        self.finish_with(completed_bytes, outcome, Some(buffer));
    }

    /// Records completion when the I/O API does not return ownership of its buffer.
    pub fn finish_without_buffer(self, completed_bytes: u64, outcome: IoOutcome) {
        self.finish_with(completed_bytes, outcome, Option::<fn() -> BufferState>::None);
    }

    fn finish_with(mut self, completed_bytes: u64, outcome: IoOutcome, buffer: Option<impl FnOnce() -> BufferState>) {
        let Some(recorded) = self.recorded.take() else {
            return;
        };
        record_finish(recorded, completed_bytes, outcome, buffer);
    }

    fn started(
        resource: &Resource,
        requested_bytes: u64,
        buffer: impl FnOnce() -> BufferState,
        start_kind: EventKind,
        finish_kind: EventKind,
    ) -> Self {
        let mut recorded = None;
        seismograph::record(EventClass::Io, || {
            let operation_id = IoOperationId::allocate();
            let resource_id = resource.id();
            let buffer = buffer();
            recorded = Some(RecordedOperation {
                operation_id,
                resource_id,
                buffer_id: buffer.id,
                requested_bytes,
                buffer_len: buffer.len,
                buffer_span_count: buffer.span_count,
                resource_kind: resource.kind,
                finish_kind,
            });
            Record::io(
                start_kind,
                IoEvent {
                    operation_id,
                    resource_id,
                    buffer_id: buffer.id,
                    requested_bytes,
                    completed_bytes: 0,
                    buffer_len: buffer.len,
                    buffer_span_count: buffer.span_count,
                    resource_kind: resource.kind,
                    outcome: IoOutcome::Pending,
                },
            )
        });
        Self { recorded }
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        let Some(recorded) = self.recorded.take() else {
            return;
        };
        record_finish(recorded, 0, IoOutcome::Canceled, Option::<fn() -> BufferState>::None);
    }
}

fn record_finish(recorded: RecordedOperation, completed_bytes: u64, outcome: IoOutcome, buffer: Option<impl FnOnce() -> BufferState>) {
    seismograph::record(EventClass::Io, || {
        let buffer = buffer.map_or(
            BufferState {
                id: recorded.buffer_id,
                len: recorded.buffer_len,
                span_count: recorded.buffer_span_count,
            },
            |buffer| buffer(),
        );
        Record::io(
            recorded.finish_kind,
            IoEvent {
                operation_id: recorded.operation_id,
                resource_id: recorded.resource_id,
                buffer_id: recorded.buffer_id.or(buffer.id),
                requested_bytes: recorded.requested_bytes,
                completed_bytes,
                buffer_len: buffer.len,
                buffer_span_count: buffer.span_count,
                resource_kind: recorded.resource_kind,
                outcome,
            },
        )
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use seismograph::recorder::event::EventPayload;
    use seismograph::recorder::{Configuration, EventBufferCapacity, RecordingPolicy};
    use seismograph::snapshot::SnapshotOptions;

    use super::*;

    #[test]
    fn recording_gates_work_and_enabled_operations_emit_pairs() {
        seismograph::recorder(Configuration::default());
        let calls = AtomicUsize::new(0);
        let resource = Resource::new(IoResourceKind::File);

        let operation = Operation::read_started(&resource, 64, || {
            calls.fetch_add(1, Ordering::Relaxed);
            BufferState::none()
        });
        operation.finish(0, IoOutcome::EndOfStream, || {
            calls.fetch_add(1, Ordering::Relaxed);
            BufferState::none()
        });

        assert_eq!(calls.load(Ordering::Relaxed), 0);

        seismograph::recorder(Configuration {
            io: RecordingPolicy::all(false),
            event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
            ..Configuration::default()
        });
        let resource = Resource::new(IoResourceKind::TcpStream);
        let operation = Operation::write_started(&resource, 8, BufferState::none);
        assert!(operation.was_recorded());
        operation.finish_without_buffer(8, IoOutcome::Success);

        let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        let events = &decoded.events.events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, EventKind::IoWriteStarted);
        assert_eq!(events[1].kind, EventKind::IoWriteFinished);
        let EventPayload::Io(start) = events[0].payload else {
            panic!("expected I/O payload");
        };
        let EventPayload::Io(finish) = events[1].payload else {
            panic!("expected I/O payload");
        };
        assert_eq!(start.operation_id, finish.operation_id);
        assert_eq!(start.resource_id, finish.resource_id);
        assert!(events[1].timestamp.ticks() >= events[0].timestamp.ticks());

        seismograph::recorder(Configuration {
            io: RecordingPolicy::default(),
            event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
            ..Configuration::default()
        });
    }
}
