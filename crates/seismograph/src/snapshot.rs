// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Process snapshot capture, source registration, and encoding.
//!
//! Format version 8 adds independently configurable cache recording.
//! [`crate::snapshot::decode()`] continues to accept versions 1 through 7.
//! Unknown future format versions are rejected.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fmt;
use std::path::Path;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::Instant;

use crate::Error;
use crate::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind};
use crate::recorder::event::{
    Address, Event, EventClock, EventKind, EventPayload, EventSequence, EventTimestamp, Events, NumericEvent, ObjectId,
};
use crate::recorder::io::{BufferId, IoEvent, IoOperationId, IoOutcome, IoResourceId, IoResourceKind};
use crate::recorder::runtime::{RuntimeEvent, RuntimeId, WorkerId};
use crate::recorder::thread::{ThreadId, ThreadLog};
use crate::recorder::{self, EventSampling, MAX_STACK_FRAMES, RecordingPolicies, RecordingPolicy, SuppressionGuard};

const MAGIC: [u8; 8] = *b"SEISMOG\0";
const FORMAT_VERSION: u16 = 8;
const HEADER_LEN: usize = 92;
const SOURCE_HEADER_LEN: usize = 24;
const EVENT_FIXED_LEN: usize = 92;
const SNAPSHOT_ARENA_CHUNK_BYTES: usize = 4 * 1024 * 1024;

static SOURCES: AtomicPtr<Source> = AtomicPtr::new(ptr::null_mut());

thread_local! {
    static ACTIVE_SNAPSHOT_ARENA: Cell<*mut SnapshotArena> = const { Cell::new(ptr::null_mut()) };
    static SNAPSHOT_ARENA_SUSPENSION_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Treatment of recorder event buffers after a snapshot captures them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EventBufferDisposition {
    /// Keeps retained events and their backing buffers.
    #[default]
    Retain,
    /// Discards retained events after capture while keeping allocated buffers for reuse.
    Clear,
    /// Discards retained events after capture and releases their backing buffers.
    Release,
}

/// Options controlling snapshot capture and recorder cleanup.
///
/// Destructive buffer dispositions take effect after runtime events are copied.
/// A later source-capture or encoding error does not restore discarded buffers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SnapshotOptions {
    /// Treatment applied to event buffers after their contents are captured.
    pub event_buffers: EventBufferDisposition,
}

/// Stable identity of a snapshot source.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct SourceId(u64);

impl SourceId {
    /// Creates a source identity from a stable numeric value.
    ///
    /// # Panics
    ///
    /// Panics when `value` is zero, which is reserved for the absence of a source.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        assert!(value != 0, "a seismograph source ID must be nonzero");
        Self(value)
    }

    /// Returns the numeric identity.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A process-lifetime source of point-in-time snapshot data.
#[derive(Debug)]
pub struct Source {
    id: SourceId,
    name: &'static str,
    schema_version: u16,
    capture: Capture,
    registered: AtomicBool,
    next: AtomicPtr<Self>,
}

/// Callback that contributes one source payload to a snapshot.
pub type Capture = for<'a> fn(SnapshotContext<'a>) -> Result<SourceData, Error>;

impl Source {
    /// Creates a source descriptor suitable for static storage.
    ///
    /// # Panics
    ///
    /// Panics when `name` is empty or `schema_version` is zero.
    #[must_use]
    pub const fn new(id: SourceId, name: &'static str, schema_version: u16, capture: Capture) -> Self {
        assert!(!name.is_empty(), "a seismograph source name must not be empty");
        assert!(schema_version != 0, "a seismograph source schema version must be nonzero");
        Self {
            id,
            name,
            schema_version,
            capture,
            registered: AtomicBool::new(false),
            next: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

/// Recorder data available while a registered source contributes its payload.
#[derive(Clone, Copy, Debug)]
pub struct SnapshotContext<'a> {
    events: &'a Events,
}

impl<'a> SnapshotContext<'a> {
    /// Returns the recorder events captured for the complete snapshot.
    #[must_use]
    pub const fn events(self) -> &'a Events {
        self.events
    }
}

/// Registers a process-lifetime snapshot source.
///
/// Repeated registration of the same source is inexpensive and has no effect.
///
/// # Panics
///
/// Panics only if the atomic registry update closure unexpectedly declines an
/// update, which the fixed `Some` result makes unreachable.
pub fn register_source(source: &'static Source) {
    if source.registered.swap(true, Ordering::AcqRel) {
        return;
    }

    SOURCES
        .fetch_update(Ordering::Release, Ordering::Acquire, |head| {
            source.next.store(head, Ordering::Relaxed);
            Some(ptr::from_ref(source).cast_mut())
        })
        .expect("the source registry update closure always returns Some");
}

/// Owned bytes returned by a snapshot source.
pub struct SourceData {
    bytes: SystemBytes,
}

impl SourceData {
    /// Creates zero-filled source storage that bypasses the global allocator.
    ///
    /// # Errors
    ///
    /// Returns an error when the system allocator cannot reserve the storage.
    pub fn zeroed(len: usize) -> Result<Self, Error> {
        SystemBytes::zeroed(len)
            .map(|bytes| Self { bytes })
            .ok_or_else(Error::allocation_failed)
    }

    /// Copies source bytes into storage that bypasses the global allocator.
    ///
    /// # Errors
    ///
    /// Returns an error when the system allocator cannot reserve the storage.
    pub fn copy_from(bytes: &[u8]) -> Result<Self, Error> {
        SystemBytes::copy_from(bytes)
            .map(|bytes| Self { bytes })
            .ok_or_else(Error::allocation_failed)
    }

    fn as_bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    /// Returns the source bytes for direct encoding.
    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        self.bytes.as_mut_slice()
    }
}

impl fmt::Debug for SourceData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceData").field("bytes", &self.bytes.len).finish_non_exhaustive()
    }
}

/// One decoded source section.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSnapshot {
    /// Stable source identity.
    pub id: SourceId,
    /// Human-readable source name.
    pub name: String,
    /// Source-owned payload schema version.
    pub schema_version: u16,
    /// Source-owned encoded payload.
    pub data: Vec<u8>,
}

/// A decoded seismograph snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DecodedSnapshot {
    /// Time spent collecting and encoding the snapshot.
    pub capture_duration_nanos: u64,
    /// Retained general-purpose runtime events.
    pub events: Events,
    /// Point-in-time data contributed by registered sources.
    pub sources: Vec<SourceSnapshot>,
}

/// An opaque encoded seismograph snapshot.
pub struct Snapshot {
    bytes: SystemBytes,
}

impl Snapshot {
    /// Returns the complete encoded snapshot.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    /// Writes the complete encoded snapshot to a file.
    ///
    /// # Errors
    ///
    /// Returns the filesystem error reported while writing `path`.
    pub fn write_file(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let _suppression = SuppressionGuard::enter();
        let path = path.as_ref();
        std::fs::write(path, self.as_bytes()).map_err(|source| Error::write_file(path.to_path_buf(), source))
    }
}

impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Snapshot").field("bytes", &self.bytes.len).finish_non_exhaustive()
    }
}

/// Captures general-purpose events and all registered snapshot sources.
///
/// # Errors
///
/// Returns an error when a source fails, source identities conflict, or the
/// system-backed output buffer cannot be allocated.
pub(crate) fn snapshot(options: SnapshotOptions) -> Result<Snapshot, Error> {
    with_snapshot_arena(|| {
        let _suppression = SuppressionGuard::enter();
        let started_at = Instant::now();
        let mut events = recorder::snapshot(options.event_buffers).unwrap_or_default();
        events.clock = EventClock::CURRENT;
        let sources = capture_sources(SnapshotContext { events: &events })?;
        encode_snapshot(&DecodedSnapshot {
            capture_duration_nanos: u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX),
            events,
            sources,
        })
    })
}

/// Decodes a seismograph snapshot.
///
/// # Errors
///
/// Returns an error when the bytes are malformed or use an unsupported format.
pub fn decode(bytes: &[u8]) -> Result<DecodedSnapshot, Error> {
    let mut reader = Reader::new(bytes);
    if reader.read(8)? != MAGIC {
        return Err(Error::invalid_format());
    }
    let format_version = reader.u16()?;
    if !(1..=FORMAT_VERSION).contains(&format_version) || reader.u16()? != 0 {
        return Err(Error::invalid_format());
    }
    let capture_duration_nanos = reader.u64()?;
    let thread_count = reader.u32()? as usize;
    let event_count = reader.u32()? as usize;
    let source_count = reader.u32()? as usize;
    let recording = decode_recording_policies(&mut reader, format_version)?;
    let clock = decode_clock(&mut reader, format_version)?;

    let threads = decode_threads(&mut reader, thread_count, format_version)?;

    let mut events = Vec::with_capacity(event_count);
    for _ in 0..event_count {
        let thread_id = ThreadId::new(reader.u64()?);
        let sequence = EventSequence::new(reader.u64()?);
        let (timestamp, kind, payload, frame_count) = if format_version >= 4 {
            decode_event_v4(&mut reader)?
        } else {
            decode_legacy_event(&mut reader, format_version)?
        };
        let mut call_stack = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            call_stack.push(Address::new(reader.u64()?));
        }
        events.push(Event {
            thread_id,
            sequence,
            timestamp,
            kind,
            payload,
            call_stack,
        });
    }

    let mut sources = Vec::with_capacity(source_count);
    for _ in 0..source_count {
        let id = SourceId::new(reader.u64()?);
        let schema_version = reader.u16()?;
        let name_len = reader.u16()? as usize;
        let data_len = usize::try_from(reader.u64()?).map_err(|_error| Error::invalid_format())?;
        if reader.u32()? != 0 {
            return Err(Error::invalid_format());
        }
        let name = std::str::from_utf8(reader.read(name_len)?)
            .map_err(|_error| Error::invalid_format())?
            .to_owned();
        let data = reader.read(data_len)?.to_vec();
        sources.push(SourceSnapshot {
            id,
            name,
            schema_version,
            data,
        });
    }
    if !reader.remaining().is_empty() {
        return Err(Error::invalid_format());
    }

    let total_events = threads.iter().map(|thread| thread.total_events).sum();
    let lost_events = threads.iter().map(|thread| thread.lost_events).sum();
    Ok(DecodedSnapshot {
        capture_duration_nanos,
        events: Events {
            clock,
            total_events,
            lost_events,
            recording,
            threads,
            events,
        },
        sources,
    })
}

fn decode_sampling(reader: &mut Reader<'_>, format_version: u16) -> Result<u64, Error> {
    if format_version < 3 {
        return Ok(1);
    }
    let sampling = reader.u32()?;
    validated_sampling(u64::from(sampling)).map(u64::from)
}

fn decode_recording_policies(reader: &mut Reader<'_>, format_version: u16) -> Result<RecordingPolicies, Error> {
    if format_version < 5 {
        let sampling =
            EventSampling::one_in(usize::try_from(decode_sampling(reader, format_version)?).map_err(|_error| Error::invalid_format())?)
                .ok_or_else(Error::invalid_format)?;
        let policy = RecordingPolicy {
            enabled: true,
            capture_backtraces: false,
            event_sampling: sampling,
        };
        return Ok(RecordingPolicies {
            allocations: policy,
            general_events: policy,
            arc_dereferences: policy,
            runtime_tasks: policy,
            io: RecordingPolicy::default(),
            cache: RecordingPolicy::default(),
        });
    }
    let allocations = decode_recording_policy(reader)?;
    let general_events = decode_recording_policy(reader)?;
    let arc_dereferences = decode_recording_policy(reader)?;
    let runtime_tasks = if format_version >= 6 {
        decode_recording_policy(reader)?
    } else {
        general_events
    };
    let io = if format_version >= 7 {
        decode_recording_policy(reader)?
    } else {
        RecordingPolicy::default()
    };
    let cache = if format_version >= 8 {
        decode_recording_policy(reader)?
    } else {
        RecordingPolicy::default()
    };
    Ok(RecordingPolicies {
        allocations,
        general_events,
        arc_dereferences,
        runtime_tasks,
        io,
        cache,
    })
}

fn decode_recording_policy(reader: &mut Reader<'_>) -> Result<RecordingPolicy, Error> {
    let enabled = reader.u8()?;
    let capture_backtraces = reader.u8()?;
    if enabled > 1 || capture_backtraces > 1 || reader.u16()? != 0 {
        return Err(Error::invalid_format());
    }
    let sampling = reader.u32()?;
    let event_sampling =
        EventSampling::one_in(usize::try_from(sampling).map_err(|_error| Error::invalid_format())?).ok_or_else(Error::invalid_format)?;
    Ok(RecordingPolicy {
        enabled: enabled != 0,
        capture_backtraces: capture_backtraces != 0,
        event_sampling,
    })
}

fn decode_clock(reader: &mut Reader<'_>, format_version: u16) -> Result<EventClock, Error> {
    if format_version < 4 {
        return Ok(EventClock::Unspecified);
    }
    let clock = EventClock::from_wire_value(reader.u16()?).ok_or_else(Error::invalid_format)?;
    if reader.u16()? != 0 || clock != EventClock::ProcessMonotonic || reader.u64()? != 1_000_000_000 {
        return Err(Error::invalid_format());
    }
    Ok(clock)
}

fn decode_event_v4(reader: &mut Reader<'_>) -> Result<(EventTimestamp, EventKind, EventPayload, usize), Error> {
    let timestamp = EventTimestamp::from_ticks(reader.u64()?);
    let kind = event_kind(reader.u8()?)?;
    let payload_tag = reader.u8()?;
    let frame_count = usize::from(reader.u8()?);
    if reader.u8()? != 0 || frame_count > MAX_STACK_FRAMES {
        return Err(Error::invalid_format());
    }
    let mut fields = [0_u64; 8];
    for field in &mut fields {
        *field = reader.u64()?;
    }
    let payload = decode_payload(payload_tag, fields)?;
    validate_payload(kind, payload)?;
    Ok((timestamp, kind, payload, frame_count))
}

fn decode_legacy_event(reader: &mut Reader<'_>, format_version: u16) -> Result<(EventTimestamp, EventKind, EventPayload, usize), Error> {
    let object_id = ObjectId::new(reader.u64()?);
    let kind = event_kind(reader.u8()?)?;
    let frame_count = usize::from(reader.u8()?);
    if reader.u16()? != 0 || frame_count > MAX_STACK_FRAMES {
        return Err(Error::invalid_format());
    }
    let payload = if format_version >= 2 {
        let event_thread_id_or_measurement = reader.u64()?;
        let heap_id = reader.u64()?;
        let address = reader.u64()?;
        let size = reader.u64()?;
        let alignment = reader.u64()?;
        let flags = reader.u64()?;
        if matches!(kind, EventKind::Allocation | EventKind::Deallocation) {
            EventPayload::Allocation(Allocation {
                allocation_id: AllocationId::new(object_id.get()),
                event_thread_id: EventThreadId::new(event_thread_id_or_measurement),
                heap_id: HeapId::new(heap_id),
                heap_kind: decode_heap_kind(flags.to_le_bytes()[0])?,
                freed_after_heap_release: flags & (1 << 8) != 0,
                address: Address::new(address),
                size,
                alignment,
            })
        } else if kind == EventKind::ChannelHighWatermark {
            if heap_id != 0 || address != 0 || size != 0 || alignment != 0 || flags != 0 {
                return Err(Error::invalid_format());
            }
            EventPayload::Numeric(NumericEvent {
                object_id,
                value: event_thread_id_or_measurement,
            })
        } else {
            if event_thread_id_or_measurement != 0 || heap_id != 0 || address != 0 || size != 0 || alignment != 0 || flags != 0 {
                return Err(Error::invalid_format());
            }
            EventPayload::Object(object_id)
        }
    } else {
        EventPayload::Object(object_id)
    };
    Ok((EventTimestamp::from_ticks(0), kind, payload, frame_count))
}

fn decode_payload(tag: u8, fields: [u64; 8]) -> Result<EventPayload, Error> {
    match tag {
        1 if fields[1..].iter().all(|&value| value == 0) => Ok(EventPayload::Object(ObjectId::new(fields[0]))),
        2 if fields[2..].iter().all(|&value| value == 0) => Ok(EventPayload::Numeric(NumericEvent {
            object_id: ObjectId::new(fields[0]),
            value: fields[1],
        })),
        3 if fields[7] == 0 => Ok(EventPayload::Allocation(Allocation {
            allocation_id: AllocationId::new(fields[0]),
            event_thread_id: EventThreadId::new(fields[1]),
            heap_id: HeapId::new(fields[2]),
            heap_kind: decode_heap_kind(fields[6].to_le_bytes()[0])?,
            freed_after_heap_release: fields[6] & (1 << 8) != 0,
            address: Address::new(fields[3]),
            size: fields[4],
            alignment: fields[5],
        })),
        4 if fields[6] == 0 && fields[7] == 0 => {
            let runtime_id = RuntimeId::from_raw(fields[0]).ok_or_else(Error::invalid_format)?;
            let worker_id = if fields[1] == 0 {
                None
            } else {
                Some(WorkerId::from_raw(fields[1]).ok_or_else(Error::invalid_format)?)
            };
            Ok(EventPayload::Runtime(RuntimeEvent {
                runtime_id,
                worker_id,
                subject_id: fields[2],
                related_id: fields[3],
                value_0: fields[4],
                value_1: fields[5],
            }))
        }
        5 if fields[5] == 0 && fields[7] >> 48 == 0 => {
            let metadata = fields[7];
            let buffer_id = match fields[2] {
                0 => None,
                value => Some(BufferId::from_raw(value).ok_or_else(Error::invalid_format)?),
            };
            Ok(EventPayload::Io(IoEvent {
                operation_id: IoOperationId::from_raw(fields[0]).ok_or_else(Error::invalid_format)?,
                resource_id: IoResourceId::from_raw(fields[1]).ok_or_else(Error::invalid_format)?,
                buffer_id,
                requested_bytes: fields[3],
                completed_bytes: fields[4],
                buffer_len: fields[6],
                buffer_span_count: u32::try_from(metadata & u64::from(u32::MAX)).map_err(|_error| Error::invalid_format())?,
                resource_kind: IoResourceKind::from_wire_value(
                    u8::try_from((metadata >> 32) & u64::from(u8::MAX)).map_err(|_error| Error::invalid_format())?,
                )
                .ok_or_else(Error::invalid_format)?,
                outcome: IoOutcome::from_wire_value(
                    u8::try_from((metadata >> 40) & u64::from(u8::MAX)).map_err(|_error| Error::invalid_format())?,
                )
                .ok_or_else(Error::invalid_format)?,
            }))
        }
        _ => Err(Error::invalid_format()),
    }
}

fn validate_payload(kind: EventKind, payload: EventPayload) -> Result<(), Error> {
    let valid = if matches!(kind, EventKind::Allocation | EventKind::Deallocation) {
        matches!(payload, EventPayload::Allocation(_))
    } else if kind == EventKind::ChannelHighWatermark || kind.class() == crate::recorder::event::EventClass::Cache {
        matches!(payload, EventPayload::Numeric(_))
    } else if kind.is_runtime() {
        matches!(payload, EventPayload::Runtime(_))
    } else if kind.is_io() {
        matches!(payload, EventPayload::Io(_))
    } else {
        matches!(payload, EventPayload::Object(_))
    };
    if valid { Ok(()) } else { Err(Error::invalid_format()) }
}

fn validated_sampling(value: u64) -> Result<u32, Error> {
    let value = usize::try_from(value).map_err(|_error| Error::invalid_format())?;
    let sampling = EventSampling::one_in(value).ok_or_else(Error::invalid_format)?;
    u32::try_from(sampling.get()).map_err(|_error| Error::invalid_format())
}

fn decode_threads(reader: &mut Reader<'_>, count: usize, format_version: u16) -> Result<Vec<ThreadLog>, Error> {
    let mut threads = Vec::with_capacity(count);
    for _ in 0..count {
        threads.push(ThreadLog {
            thread_id: ThreadId::new(reader.u64()?),
            total_events: reader.u64()?,
            lost_events: reader.u64()?,
            name: if format_version >= 2 {
                let len = usize::from(reader.u16()?);
                String::from_utf8(reader.read(len)?.to_vec()).map_err(|_error| Error::invalid_format())?
            } else {
                String::new()
            },
        });
    }
    Ok(threads)
}

fn capture_sources(context: SnapshotContext<'_>) -> Result<Vec<SourceSnapshot>, Error> {
    capture_sources_from(SOURCES.load(Ordering::Acquire), context)
}

fn capture_sources_from(mut source: *mut Source, context: SnapshotContext<'_>) -> Result<Vec<SourceSnapshot>, Error> {
    let mut snapshots = Vec::new();
    while !source.is_null() {
        // SAFETY: registered sources have process lifetime and their next link
        // is initialized before they become visible through SOURCES.
        let descriptor = unsafe { &*source };
        if snapshots.iter().any(|snapshot: &SourceSnapshot| snapshot.id == descriptor.id) {
            return Err(Error::duplicate_source(descriptor.id));
        }
        let data = (descriptor.capture)(context).map_err(|error| Error::source_failed(descriptor.id, error))?;
        snapshots.push(SourceSnapshot {
            id: descriptor.id,
            name: descriptor.name.to_owned(),
            schema_version: descriptor.schema_version,
            data: data.as_bytes().to_vec(),
        });
        source = descriptor.next.load(Ordering::Acquire);
    }
    Ok(snapshots)
}

fn encode_snapshot(snapshot: &DecodedSnapshot) -> Result<Snapshot, Error> {
    let events_len = snapshot.events.events.iter().try_fold(0usize, |total, event| {
        EVENT_FIXED_LEN
            .checked_add(event.call_stack.len().checked_mul(8)?)
            .and_then(|len| total.checked_add(len))
    });
    let sources_len = snapshot.sources.iter().try_fold(0usize, |total, source| {
        let len = SOURCE_HEADER_LEN.checked_add(source.name.len())?.checked_add(source.data.len())?;
        total.checked_add(len)
    });
    let threads_len = snapshot
        .events
        .threads
        .iter()
        .try_fold(0usize, |total, thread| total.checked_add(26)?.checked_add(thread.name.len()));
    let len = HEADER_LEN
        .checked_add(threads_len.ok_or_else(Error::allocation_failed)?)
        .and_then(|len| len.checked_add(events_len?))
        .and_then(|len| len.checked_add(sources_len?))
        .ok_or_else(Error::allocation_failed)?;
    let mut bytes = SystemBytes::zeroed(len).ok_or_else(Error::allocation_failed)?;
    let mut writer = Writer::new(bytes.as_mut_slice());
    writer.write(&MAGIC)?;
    writer.u16(FORMAT_VERSION)?;
    writer.u16(0)?;
    writer.u64(snapshot.capture_duration_nanos)?;
    writer.u32(u32::try_from(snapshot.events.threads.len()).map_err(|_error| Error::invalid_format())?)?;
    writer.u32(u32::try_from(snapshot.events.events.len()).map_err(|_error| Error::invalid_format())?)?;
    writer.u32(u32::try_from(snapshot.sources.len()).map_err(|_error| Error::invalid_format())?)?;
    encode_recording_policy(&mut writer, snapshot.events.recording.allocations)?;
    encode_recording_policy(&mut writer, snapshot.events.recording.general_events)?;
    encode_recording_policy(&mut writer, snapshot.events.recording.arc_dereferences)?;
    encode_recording_policy(&mut writer, snapshot.events.recording.runtime_tasks)?;
    encode_recording_policy(&mut writer, snapshot.events.recording.io)?;
    encode_recording_policy(&mut writer, snapshot.events.recording.cache)?;
    if snapshot.events.clock != EventClock::CURRENT {
        return Err(Error::invalid_format());
    }
    writer.u16(snapshot.events.clock.wire_value())?;
    writer.u16(0)?;
    writer.u64(snapshot.events.clock.ticks_per_second().ok_or_else(Error::invalid_format)?)?;
    for thread in &snapshot.events.threads {
        writer.u64(thread.thread_id.get())?;
        writer.u64(thread.total_events)?;
        writer.u64(thread.lost_events)?;
        writer.u16(u16::try_from(thread.name.len()).map_err(|_error| Error::invalid_format())?)?;
        writer.write(thread.name.as_bytes())?;
    }
    for event in &snapshot.events.events {
        validate_payload(event.kind, event.payload)?;
        writer.u64(event.thread_id.get())?;
        writer.u64(event.sequence.get())?;
        writer.u64(event.timestamp.ticks())?;
        writer.u8(event.kind.wire_value())?;
        writer.u8(payload_tag(event.payload))?;
        writer.u8(u8::try_from(event.call_stack.len()).map_err(|_error| Error::invalid_format())?)?;
        writer.u8(0)?;
        for field in encode_payload(event.payload) {
            writer.u64(field)?;
        }
        for &frame in &event.call_stack {
            writer.u64(frame.get())?;
        }
    }
    for source in &snapshot.sources {
        writer.u64(source.id.get())?;
        writer.u16(source.schema_version)?;
        writer.u16(u16::try_from(source.name.len()).map_err(|_error| Error::invalid_format())?)?;
        writer.u64(u64::try_from(source.data.len()).map_err(|_error| Error::invalid_format())?)?;
        writer.u32(0)?;
        writer.write(source.name.as_bytes())?;
        writer.write(&source.data)?;
    }
    debug_assert!(writer.remaining().is_empty(), "encoded length accounts for every written field");
    Ok(Snapshot { bytes })
}

fn encode_recording_policy(writer: &mut Writer<'_>, policy: RecordingPolicy) -> Result<(), Error> {
    writer.u8(u8::from(policy.enabled))?;
    writer.u8(u8::from(policy.capture_backtraces))?;
    writer.u16(0)?;
    writer.u32(validated_sampling(u64::try_from(policy.event_sampling.get()).unwrap_or(u64::MAX))?)
}

fn event_kind(value: u8) -> Result<EventKind, Error> {
    EventKind::from_wire_value(value).ok_or_else(Error::invalid_format)
}

const fn payload_tag(payload: EventPayload) -> u8 {
    match payload {
        EventPayload::Object(_) => 1,
        EventPayload::Numeric(_) => 2,
        EventPayload::Allocation(_) => 3,
        EventPayload::Runtime(_) => 4,
        EventPayload::Io(_) => 5,
    }
}

const fn encode_payload(payload: EventPayload) -> [u64; 8] {
    match payload {
        EventPayload::Object(object_id) => [object_id.get(), 0, 0, 0, 0, 0, 0, 0],
        EventPayload::Numeric(payload) => [payload.object_id.get(), payload.value, 0, 0, 0, 0, 0, 0],
        EventPayload::Allocation(allocation) => [
            allocation.allocation_id.get(),
            allocation.event_thread_id.get(),
            allocation.heap_id.get(),
            allocation.address.get(),
            allocation.size,
            allocation.alignment,
            encode_heap_kind(allocation.heap_kind) | (if allocation.freed_after_heap_release { 1 << 8 } else { 0 }),
            0,
        ],
        EventPayload::Runtime(runtime) => [
            runtime.runtime_id.get(),
            match runtime.worker_id {
                Some(worker_id) => worker_id.get(),
                None => 0,
            },
            runtime.subject_id,
            runtime.related_id,
            runtime.value_0,
            runtime.value_1,
            0,
            0,
        ],
        EventPayload::Io(io) => [
            io.operation_id.get(),
            io.resource_id.get(),
            match io.buffer_id {
                Some(buffer_id) => buffer_id.get(),
                None => 0,
            },
            io.requested_bytes,
            io.completed_bytes,
            0,
            io.buffer_len,
            io.buffer_span_count as u64 | ((io.resource_kind.wire_value() as u64) << 32) | ((io.outcome.wire_value() as u64) << 40),
        ],
    }
}

const fn encode_heap_kind(kind: HeapKind) -> u64 {
    match kind {
        HeapKind::General => 1,
        HeapKind::Bump => 2,
        HeapKind::Thread => 3,
    }
}

fn decode_heap_kind(value: u8) -> Result<HeapKind, Error> {
    match value {
        1 => Ok(HeapKind::General),
        2 => Ok(HeapKind::Bump),
        3 => Ok(HeapKind::Thread),
        _ => Err(Error::invalid_format()),
    }
}

struct SystemBytes {
    address: NonNull<u8>,
    len: usize,
}

// SAFETY: SystemBytes exclusively owns a System allocation and exposes no
// thread-affine state.
unsafe impl Send for SystemBytes {}
// SAFETY: shared access exposes only initialized immutable bytes.
unsafe impl Sync for SystemBytes {}

impl SystemBytes {
    fn zeroed(len: usize) -> Option<Self> {
        if len == 0 {
            return Some(Self {
                address: NonNull::dangling(),
                len,
            });
        }
        let layout = Layout::array::<u8>(len).ok()?;
        // SAFETY: layout is nonzero and valid. The returned pointer is owned by
        // this value and released with the same allocator and layout.
        let address = NonNull::new(unsafe { System.alloc_zeroed(layout) })?;
        Some(Self { address, len })
    }

    fn copy_from(bytes: &[u8]) -> Option<Self> {
        let mut result = Self::zeroed(bytes.len())?;
        result.as_mut_slice().copy_from_slice(bytes);
        Some(result)
    }

    fn as_slice(&self) -> &[u8] {
        // SAFETY: address owns len initialized bytes for this value's lifetime.
        unsafe { std::slice::from_raw_parts(self.address.as_ptr(), self.len) }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: this value exclusively owns len initialized bytes.
        unsafe { std::slice::from_raw_parts_mut(self.address.as_ptr(), self.len) }
    }
}

impl Drop for SystemBytes {
    fn drop(&mut self) {
        if self.len == 0 {
            return;
        }
        let layout = Layout::array::<u8>(self.len).expect("the allocation was created from this representable layout");
        // SAFETY: address was allocated by System with this exact layout.
        unsafe { System.dealloc(self.address.as_ptr(), layout) };
    }
}

struct SnapshotArenaChunk {
    previous: *mut Self,
    layout: Layout,
    cursor: usize,
    dedicated: bool,
}

struct SnapshotArena {
    head: *mut SnapshotArenaChunk,
    parent: *mut Self,
}

impl SnapshotArena {
    const fn new() -> Self {
        Self {
            head: ptr::null_mut(),
            parent: ptr::null_mut(),
        }
    }

    #[expect(
        clippy::cast_ptr_alignment,
        reason = "System allocated the mapping with SnapshotArenaChunk alignment"
    )]
    fn allocate(&mut self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(1);
        // SAFETY: a non-null head points to a live chunk owned by this arena.
        let head_is_reusable = !self.head.is_null() && !unsafe { (*self.head).dedicated };
        if head_is_reusable {
            // SAFETY: the head is a live, exclusively owned arena chunk.
            let address = unsafe { allocate_from_snapshot_chunk(self.head, size, layout.align()) };
            if !address.is_null() {
                return address;
            }
        }
        // Layout guarantees that size rounded up to alignment fits in isize;
        // adding the small chunk header therefore fits in usize.
        let required = size_of::<SnapshotArenaChunk>() + layout.align() - 1 + size;
        let dedicated = required > SNAPSHOT_ARENA_CHUNK_BYTES / 2;
        let bytes = if dedicated {
            required
        } else {
            SNAPSHOT_ARENA_CHUNK_BYTES.max(required)
        };
        let Ok(mapping_layout) = Layout::from_size_align(bytes, align_of::<SnapshotArenaChunk>()) else {
            return ptr::null_mut();
        };
        // SAFETY: mapping_layout is nonzero and valid, and the resulting
        // allocation is owned by this arena.
        let mapping = unsafe { System.alloc(mapping_layout) };
        if mapping.is_null() {
            return ptr::null_mut();
        }
        let chunk = mapping.cast::<SnapshotArenaChunk>();
        // SAFETY: the mapping is writable and large enough for this header.
        unsafe {
            chunk.write(SnapshotArenaChunk {
                previous: self.head,
                layout: mapping_layout,
                cursor: size_of::<SnapshotArenaChunk>(),
                dedicated,
            });
        }
        self.head = chunk;
        // SAFETY: chunk was initialized above and is exclusively owned.
        unsafe { allocate_from_snapshot_chunk(chunk, size, layout.align()) }
    }

    fn deallocate(&mut self, address: *mut u8) -> bool {
        let mut link = &raw mut self.head;
        loop {
            // SAFETY: link points to the arena head or a live chunk's next field.
            let chunk = unsafe { *link };
            if chunk.is_null() {
                break;
            }
            let start = chunk.addr();
            // SAFETY: chunk is a live node owned by this arena.
            let chunk_layout = unsafe { (*chunk).layout };
            let end = start.saturating_add(chunk_layout.size());
            if address.addr() >= start && address.addr() < end {
                // SAFETY: chunk is a live node owned by this arena.
                let dedicated = unsafe { (*chunk).dedicated };
                if dedicated {
                    // SAFETY: chunk is live, so its previous link is readable.
                    let previous = unsafe { (*chunk).previous };
                    // SAFETY: link identifies the pointer that currently owns chunk.
                    unsafe { *link = previous };
                    // SAFETY: dedicated chunks can be released independently and
                    // were allocated by System with chunk_layout.
                    unsafe { System.dealloc(chunk.cast(), chunk_layout) };
                }
                return true;
            }
            // SAFETY: link points into a live arena-owned chunk.
            link = unsafe { &raw mut (*chunk).previous };
        }
        if self.parent.is_null() {
            false
        } else {
            // SAFETY: nested arenas are stack-scoped and the parent outlives
            // this child activation.
            unsafe { (*self.parent).deallocate(address) }
        }
    }
}

impl Drop for SnapshotArena {
    fn drop(&mut self) {
        let mut chunk = self.head;
        while !chunk.is_null() {
            // SAFETY: the list contains live mappings owned by this arena.
            let previous = unsafe { (*chunk).previous };
            // SAFETY: chunk remains live until it is deallocated below.
            let layout = unsafe { (*chunk).layout };
            // SAFETY: chunk was allocated by System with layout.
            unsafe { System.dealloc(chunk.cast(), layout) };
            chunk = previous;
        }
    }
}

struct SnapshotArenaActivation {
    previous: *mut SnapshotArena,
}

impl Drop for SnapshotArenaActivation {
    fn drop(&mut self) {
        let _ = ACTIVE_SNAPSHOT_ARENA.try_with(|active| active.set(self.previous));
    }
}

struct SnapshotArenaSuspension;

impl Drop for SnapshotArenaSuspension {
    fn drop(&mut self) {
        let _ = SNAPSHOT_ARENA_SUSPENSION_DEPTH.try_with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

fn with_snapshot_arena<R>(operation: impl FnOnce() -> R) -> R {
    let mut arena = SnapshotArena::new();
    let previous = ACTIVE_SNAPSHOT_ARENA
        .try_with(|active| active.replace(ptr::from_mut(&mut arena)))
        .unwrap_or(ptr::null_mut());
    arena.parent = previous;
    let _activation = SnapshotArenaActivation { previous };
    operation()
}

/// Runs an operation with snapshot-arena allocation temporarily disabled.
///
/// Snapshot sources must use this around updates to process-lifetime caches;
/// otherwise cache storage allocated during capture is reclaimed when the
/// current snapshot finishes.
#[doc(hidden)]
pub fn with_snapshot_arena_suspended<R>(operation: impl FnOnce() -> R) -> R {
    let _suspension = SNAPSHOT_ARENA_SUSPENSION_DEPTH
        .try_with(|depth| {
            depth.set(depth.get().saturating_add(1));
            SnapshotArenaSuspension
        })
        .ok();
    operation()
}

/// Returns whether snapshot-arena allocation is suspended on this thread.
#[doc(hidden)]
#[must_use]
pub fn snapshot_arena_allocation_suspended() -> bool {
    SNAPSHOT_ARENA_SUSPENSION_DEPTH.try_with(|depth| depth.get() != 0).unwrap_or(true)
}

/// Allocates from the active snapshot arena, when snapshotting this thread.
#[doc(hidden)]
#[must_use]
pub fn snapshot_arena_allocate(layout: Layout) -> Option<*mut u8> {
    if snapshot_arena_allocation_suspended() {
        return None;
    }
    ACTIVE_SNAPSHOT_ARENA
        .try_with(|active| {
            let arena = active.get();
            if arena.is_null() {
                None
            } else {
                // SAFETY: the TLS pointer is installed only while its stack-owned
                // arena remains live on this thread.
                Some(unsafe { (*arena).allocate(layout) })
            }
        })
        .unwrap_or(None)
}

/// Releases or recognizes storage owned by the active snapshot arena.
#[doc(hidden)]
pub fn snapshot_arena_deallocate(address: *mut u8) -> bool {
    ACTIVE_SNAPSHOT_ARENA
        .try_with(|active| {
            let arena = active.get();
            if arena.is_null() {
                false
            } else {
                // SAFETY: the TLS pointer is installed only while its stack-owned
                // arena remains live on this thread.
                unsafe { (*arena).deallocate(address) }
            }
        })
        .unwrap_or(false)
}

unsafe fn allocate_from_snapshot_chunk(chunk: *mut SnapshotArenaChunk, size: usize, alignment: usize) -> *mut u8 {
    // SAFETY: the caller guarantees that chunk identifies a live arena chunk.
    let cursor = unsafe { (*chunk).cursor };
    let base = chunk.addr();
    // SAFETY: the caller guarantees that chunk identifies a live arena chunk.
    let chunk_size = unsafe { (*chunk).layout.size() };
    let Some((aligned_address, end)) = snapshot_chunk_address(base, cursor, size, alignment, chunk_size) else {
        return ptr::null_mut();
    };
    // SAFETY: the arena has exclusive access to the chunk cursor.
    unsafe { (*chunk).cursor = end };
    chunk.cast::<u8>().with_addr(aligned_address)
}

fn snapshot_chunk_address(base: usize, cursor: usize, size: usize, alignment: usize, chunk_size: usize) -> Option<(usize, usize)> {
    let current = base.checked_add(cursor)?;
    let aligned_address = current.checked_add(alignment - 1)? & !(alignment - 1);
    let end_address = aligned_address.checked_add(size)?;
    let end = end_address.checked_sub(base)?;
    (end <= chunk_size).then_some((aligned_address, end))
}

struct Writer<'a> {
    remaining: &'a mut [u8],
}

impl<'a> Writer<'a> {
    fn new(bytes: &'a mut [u8]) -> Self {
        Self { remaining: bytes }
    }

    fn remaining(&self) -> &[u8] {
        self.remaining
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let remaining = std::mem::take(&mut self.remaining);
        let Some((destination, tail)) = remaining.split_at_mut_checked(bytes.len()) else {
            return Err(Error::invalid_format());
        };
        destination.copy_from_slice(bytes);
        self.remaining = tail;
        Ok(())
    }

    fn u8(&mut self, value: u8) -> Result<(), Error> {
        self.write(&[value])
    }

    fn u16(&mut self, value: u16) -> Result<(), Error> {
        self.write(&value.to_le_bytes())
    }

    fn u32(&mut self, value: u32) -> Result<(), Error> {
        self.write(&value.to_le_bytes())
    }

    fn u64(&mut self, value: u64) -> Result<(), Error> {
        self.write(&value.to_le_bytes())
    }
}

struct Reader<'a> {
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    const fn remaining(&self) -> &'a [u8] {
        self.remaining
    }

    fn read(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let Some((value, remaining)) = self.remaining.split_at_checked(len) else {
            return Err(Error::invalid_format());
        };
        self.remaining = remaining;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.read(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, Error> {
        let bytes = self.read(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let bytes = self.read(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        let bytes = self.read(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::assertions_on_result_states,
        reason = "malformed-input matrices assert rejection while detailed error text is covered separately"
    )]

    use super::*;

    static TEST_SOURCE: Source = Source::new(SourceId::new(7), "test", 3, capture_test_source);

    fn capture_test_source(context: SnapshotContext<'_>) -> Result<SourceData, Error> {
        assert!(
            context
                .events()
                .events
                .iter()
                .any(|event| event.object_id() == Some(ObjectId::new(42)))
        );
        SourceData::copy_from(b"payload")
    }

    #[test]
    fn snapshot_round_trips_events_and_sources() {
        let _test = recorder::TEST_LOCK.lock().unwrap();
        register_source(&TEST_SOURCE);
        recorder::configure(recorder::Configuration {
            arc_dereferences: recorder::RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        });
        recorder::record(recorder::event::EventClass::ArcDereference, || {
            recorder::event::Record::object(EventKind::ArcDeref, ObjectId::new(42))
        });

        let snapshot = snapshot(SnapshotOptions::default()).unwrap();
        let decoded = decode(snapshot.as_bytes()).unwrap();

        assert_eq!(decoded.events.recording.arc_dereferences.event_sampling.get(), 1);
        assert!(
            decoded
                .events
                .events
                .iter()
                .any(|event| event.object_id() == Some(ObjectId::new(42)))
        );
        assert!(decoded.sources.iter().any(|source| {
            source.id == SourceId::new(7) && source.name == "test" && source.schema_version == 3 && source.data == b"payload"
        }));
        recorder::configure(recorder::Configuration::default());
    }

    #[test]
    fn snapshot_round_trips_independent_recording_policies() {
        let recording = recorder::RecordingPolicies {
            allocations: recorder::RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                event_sampling: recorder::EventSampling::one_in(16).expect("valid test sampling"),
            },
            general_events: recorder::RecordingPolicy {
                enabled: false,
                capture_backtraces: true,
                event_sampling: recorder::EventSampling::one_in(4).expect("valid test sampling"),
            },
            arc_dereferences: recorder::RecordingPolicy {
                enabled: true,
                capture_backtraces: false,
                event_sampling: recorder::EventSampling::one_in(100).expect("valid test sampling"),
            },
            runtime_tasks: recorder::RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                event_sampling: recorder::EventSampling::one_in(8).expect("valid test sampling"),
            },
            io: recorder::RecordingPolicy {
                enabled: true,
                capture_backtraces: false,
                event_sampling: recorder::EventSampling::one_in(2).expect("valid test sampling"),
            },
            cache: recorder::RecordingPolicy {
                enabled: true,
                capture_backtraces: false,
                event_sampling: recorder::EventSampling::one_in(32).expect("valid test sampling"),
            },
        };
        let snapshot = encode_snapshot(&DecodedSnapshot {
            capture_duration_nanos: 1,
            events: Events {
                clock: EventClock::CURRENT,
                recording,
                ..Events::default()
            },
            sources: Vec::new(),
        })
        .unwrap();

        assert_eq!(decode(snapshot.as_bytes()).unwrap().events.recording, recording);
    }

    #[test]
    fn version_four_round_trips_timestamp_and_runtime_payload() {
        let event = Event {
            thread_id: ThreadId::new(7),
            sequence: EventSequence::new(9),
            timestamp: EventTimestamp::from_ticks(123_456),
            kind: EventKind::TaskPollFinished,
            payload: EventPayload::Runtime(RuntimeEvent {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: Some(WorkerId::from_raw(2).unwrap()),
                subject_id: 3,
                related_id: 4,
                value_0: 5,
                value_1: 6,
            }),
            call_stack: vec![Address::new(0x1234)],
        };
        let snapshot = encode_snapshot(&DecodedSnapshot {
            capture_duration_nanos: 1,
            events: Events {
                clock: EventClock::CURRENT,
                total_events: 1,
                lost_events: 0,
                recording: RecordingPolicies::default(),
                threads: vec![ThreadLog {
                    thread_id: ThreadId::new(7),
                    total_events: 1,
                    lost_events: 0,
                    name: "worker".to_owned(),
                }],
                events: vec![event.clone()],
            },
            sources: Vec::new(),
        })
        .unwrap();

        assert_eq!(decode(snapshot.as_bytes()).unwrap().events.events, vec![event]);
    }

    #[test]
    fn current_snapshot_round_trips_lock_poison_events() {
        let kinds = [EventKind::LockPoisoned, EventKind::LockPoisonObserved, EventKind::LockPoisonCleared];
        let events = kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| Event {
                thread_id: ThreadId::new(7),
                sequence: EventSequence::new(index as u64),
                timestamp: EventTimestamp::from_ticks(index as u64),
                kind,
                payload: EventPayload::Object(ObjectId::new(42)),
                call_stack: Vec::new(),
            })
            .collect::<Vec<_>>();
        let snapshot = encode_snapshot(&DecodedSnapshot {
            capture_duration_nanos: 1,
            events: Events {
                clock: EventClock::CURRENT,
                total_events: events.len() as u64,
                recording: RecordingPolicies::default(),
                events: events.clone(),
                ..Events::default()
            },
            sources: Vec::new(),
        })
        .unwrap();

        assert_eq!(decode(snapshot.as_bytes()).unwrap().events.events, events);
    }

    #[test]
    fn version_one_events_decode_without_names_or_allocation_fields() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&7_u64.to_le_bytes());
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&42_u64.to_le_bytes());
        bytes.push(EventKind::ArcDeref.wire_value());
        bytes.push(0);
        bytes.extend_from_slice(&0_u16.to_le_bytes());

        let decoded = decode(&bytes).unwrap();
        assert_eq!(
            (
                decoded.events.clock,
                decoded.events.events[0].timestamp,
                decoded.events.events[0].object_id(),
            ),
            (EventClock::Unspecified, EventTimestamp::from_ticks(0), Some(ObjectId::new(42)),)
        );
    }

    #[test]
    fn version_three_measurements_decode_into_numeric_payloads() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&3_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&7_u64.to_le_bytes());
        bytes.extend_from_slice(&1_u64.to_le_bytes());
        bytes.extend_from_slice(&42_u64.to_le_bytes());
        bytes.push(EventKind::ChannelHighWatermark.wire_value());
        bytes.push(0);
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&9_u64.to_le_bytes());
        bytes.extend_from_slice(&[0; 5 * 8]);

        let decoded = decode(&bytes).unwrap();

        assert_eq!(
            (
                decoded.events.clock,
                decoded.events.events[0].object_id(),
                decoded.events.events[0].measurement(),
            ),
            (EventClock::Unspecified, Some(ObjectId::new(42)), Some(9))
        );
    }

    #[test]
    fn snapshot_arena_recognizes_its_allocations() {
        with_snapshot_arena(|| {
            let layout = Layout::from_size_align(128, 64).unwrap();
            let address = snapshot_arena_allocate(layout).unwrap();
            assert_eq!(address.addr() % 64, 0);
            assert!(snapshot_arena_deallocate(address));
        });
    }

    #[test]
    fn snapshot_arena_can_be_suspended_for_persistent_allocations() {
        with_snapshot_arena(|| {
            let layout = Layout::from_size_align(128, 16).unwrap();
            let address = snapshot_arena_allocate(layout).unwrap();

            with_snapshot_arena_suspended(|| {
                assert_eq!(snapshot_arena_allocate(layout), None);
                assert!(snapshot_arena_deallocate(address));
            });

            assert!(snapshot_arena_allocate(layout).is_some());
        });
    }

    #[test]
    fn source_storage_debug_and_file_persistence_work() {
        fn capture(_context: SnapshotContext<'_>) -> Result<SourceData, Error> {
            SourceData::copy_from(b"source")
        }

        let source = Source::new(SourceId::new(99), "local", 1, capture);
        assert!(format!("{source:?}").contains("Source"));
        assert_eq!(
            (source.capture)(SnapshotContext {
                events: &Events::default()
            })
            .unwrap()
            .as_bytes(),
            b"source"
        );

        let mut data = SourceData::zeroed(3).unwrap();
        data.as_mut_bytes().copy_from_slice(b"abc");
        assert!(format!("{data:?}").contains("bytes: 3"));

        let snapshot = encode_snapshot(&DecodedSnapshot {
            capture_duration_nanos: 0,
            events: Events {
                clock: EventClock::CURRENT,
                ..Events::default()
            },
            sources: Vec::new(),
        })
        .unwrap();
        assert!(format!("{snapshot:?}").contains("bytes"));
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join(format!("seismograph-snapshot-{}.bin", std::process::id()));
        snapshot.write_file(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), snapshot.as_bytes());
        std::fs::remove_file(&path).unwrap();
        assert!(snapshot.write_file(Path::new(env!("CARGO_MANIFEST_DIR"))).is_err());
    }

    #[test]
    fn current_format_round_trips_all_payload_and_heap_shapes() {
        let allocation = |sequence, heap_kind| Event {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind: EventKind::Allocation,
            payload: EventPayload::Allocation(Allocation {
                allocation_id: AllocationId::new(sequence),
                event_thread_id: EventThreadId::new(2),
                heap_id: HeapId::new(3),
                heap_kind,
                freed_after_heap_release: heap_kind == HeapKind::Bump,
                address: Address::new(4),
                size: 5,
                alignment: 8,
            }),
            call_stack: Vec::new(),
        };
        let events = vec![
            Event {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(1),
                timestamp: EventTimestamp::from_ticks(1),
                kind: EventKind::ChannelHighWatermark,
                payload: EventPayload::Numeric(NumericEvent {
                    object_id: ObjectId::new(1),
                    value: 7,
                }),
                call_stack: Vec::new(),
            },
            allocation(2, HeapKind::General),
            allocation(3, HeapKind::Bump),
            allocation(4, HeapKind::Thread),
            Event {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(5),
                timestamp: EventTimestamp::from_ticks(5),
                kind: EventKind::IoReadFinished,
                payload: EventPayload::Io(IoEvent {
                    operation_id: IoOperationId::from_raw(1).unwrap(),
                    resource_id: IoResourceId::from_raw(2).unwrap(),
                    buffer_id: Some(BufferId::from_raw(3).unwrap()),
                    requested_bytes: 4,
                    completed_bytes: 5,
                    buffer_len: 6,
                    buffer_span_count: 7,
                    resource_kind: IoResourceKind::TcpStream,
                    outcome: IoOutcome::Success,
                }),
                call_stack: Vec::new(),
            },
        ];
        let decoded = DecodedSnapshot {
            capture_duration_nanos: 1,
            events: Events {
                clock: EventClock::CURRENT,
                events,
                ..Events::default()
            },
            sources: Vec::new(),
        };
        let encoded = encode_snapshot(&decoded).unwrap();
        assert_eq!(decode(encoded.as_bytes()).unwrap(), decoded);
    }

    #[test]
    fn malformed_headers_policies_clocks_sources_and_payloads_are_rejected() {
        let mut invalid_magic = vec![0; HEADER_LEN];
        assert!(decode(&invalid_magic).is_err());

        invalid_magic[..8].copy_from_slice(&MAGIC);
        invalid_magic[8..10].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert!(decode(&invalid_magic).is_err());

        let mut invalid_policy = vec![0; HEADER_LEN];
        invalid_policy[..8].copy_from_slice(&MAGIC);
        invalid_policy[8..10].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        invalid_policy[32] = 2;
        assert!(decode(&invalid_policy).is_err());

        let mut invalid_clock = vec![0; HEADER_LEN];
        invalid_clock[..8].copy_from_slice(&MAGIC);
        invalid_clock[8..10].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        for offset in [32, 40, 48, 56, 64, 72] {
            invalid_clock[offset + 4..offset + 8].copy_from_slice(&1_u32.to_le_bytes());
        }
        invalid_clock[80..82].copy_from_slice(&2_u16.to_le_bytes());
        assert!(decode(&invalid_clock).is_err());
        invalid_clock[64..66].copy_from_slice(&EventClock::CURRENT.wire_value().to_le_bytes());
        invalid_clock[66..68].copy_from_slice(&1_u16.to_le_bytes());
        assert!(decode(&invalid_clock).is_err());

        let valid = encode_snapshot(&DecodedSnapshot {
            capture_duration_nanos: 0,
            events: Events {
                clock: EventClock::CURRENT,
                ..Events::default()
            },
            sources: Vec::new(),
        })
        .unwrap();
        let mut trailing = valid.as_bytes().to_vec();
        trailing.push(1);
        assert!(decode(&trailing).is_err());

        let mut invalid_source = valid.as_bytes().to_vec();
        invalid_source[28..32].copy_from_slice(&1_u32.to_le_bytes());
        invalid_source.extend_from_slice(&[0; SOURCE_HEADER_LEN]);
        invalid_source[HEADER_LEN..HEADER_LEN + 8].copy_from_slice(&1_u64.to_le_bytes());
        invalid_source[HEADER_LEN + 20] = 1;
        assert!(decode(&invalid_source).is_err());

        for (tag, fields) in [
            (1, [0, 1, 0, 0, 0, 0, 0, 0]),
            (2, [0, 0, 1, 0, 0, 0, 0, 0]),
            (3, [0, 0, 0, 0, 0, 0, 0, 1]),
            (3, [0, 0, 0, 0, 0, 0, 9, 0]),
            (4, [0; 8]),
            (4, [1, 0, 0, 0, 0, 0, 1, 0]),
            (9, [0; 8]),
        ] {
            assert!(decode_payload(tag, fields).is_err());
        }
        assert!(decode_heap_kind(0).is_err());
    }

    #[test]
    fn legacy_allocation_and_object_validation_paths_are_decoded() {
        fn legacy_event(kind: EventKind, fields: [u64; 6]) -> Vec<u8> {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&1_u64.to_le_bytes());
            bytes.push(kind.wire_value());
            bytes.push(0);
            bytes.extend_from_slice(&0_u16.to_le_bytes());
            for field in fields {
                bytes.extend_from_slice(&field.to_le_bytes());
            }
            bytes
        }

        let allocation = decode_legacy_event(
            &mut Reader::new(&legacy_event(EventKind::Allocation, [2, 3, 4, 5, 8, 2 | (1 << 8)])),
            3,
        )
        .unwrap();
        assert!(matches!(
            allocation.2,
            EventPayload::Allocation(Allocation {
                heap_kind: HeapKind::Bump,
                freed_after_heap_release: true,
                ..
            })
        ));

        assert!(
            decode_legacy_event(
                &mut Reader::new(&legacy_event(EventKind::ChannelHighWatermark, [7, 1, 0, 0, 0, 0])),
                3,
            )
            .is_err()
        );
        assert!(decode_legacy_event(&mut Reader::new(&legacy_event(EventKind::MutexAccess, [1, 0, 0, 0, 0, 0])), 3,).is_err());
        assert_eq!(
            decode_legacy_event(&mut Reader::new(&legacy_event(EventKind::MutexAccess, [0; 6])), 3,)
                .unwrap()
                .2,
            EventPayload::Object(ObjectId::new(1))
        );
    }

    #[test]
    fn payload_validation_rejects_mismatched_event_kinds() {
        let allocation = EventPayload::Allocation(Allocation {
            allocation_id: AllocationId::new(1),
            event_thread_id: EventThreadId::new(2),
            heap_id: HeapId::new(3),
            heap_kind: HeapKind::General,
            freed_after_heap_release: false,
            address: Address::new(4),
            size: 5,
            alignment: 8,
        });
        let numeric = EventPayload::Numeric(NumericEvent {
            object_id: ObjectId::new(1),
            value: 2,
        });
        let object = EventPayload::Object(ObjectId::new(1));
        assert!(validate_payload(EventKind::Allocation, object).is_err());
        assert!(validate_payload(EventKind::ChannelHighWatermark, allocation).is_err());
        assert!(validate_payload(EventKind::TaskSpawned, numeric).is_err());
        assert!(
            encode_snapshot(&DecodedSnapshot {
                capture_duration_nanos: 0,
                events: Events::default(),
                sources: Vec::new(),
            })
            .is_err()
        );
    }

    #[test]
    fn system_bytes_and_arena_cover_empty_dedicated_and_parent_storage() {
        let empty = SystemBytes::zeroed(0).unwrap();
        assert_eq!(empty.as_slice(), &[]);
        drop(empty);

        let mut arena = SnapshotArena::new();
        let small = arena.allocate(Layout::from_size_align(128, 16).unwrap());
        assert!(!small.is_null());
        let reused = arena.allocate(Layout::from_size_align(128, 16).unwrap());
        assert!(!reused.is_null());
        assert!(arena.deallocate(reused));
        assert!(!arena.deallocate(NonNull::<u8>::dangling().as_ptr()));

        let mut dedicated_arena = SnapshotArena::new();
        let dedicated = dedicated_arena.allocate(Layout::from_size_align(SNAPSHOT_ARENA_CHUNK_BYTES / 2, 16).unwrap());
        assert!(!dedicated.is_null());
        // SAFETY: a non-null arena head points to its live chunk.
        assert!(unsafe { (*dedicated_arena.head).dedicated });
        assert!(dedicated_arena.deallocate(dedicated));

        let shared = Layout::from_size_align(SNAPSHOT_ARENA_CHUNK_BYTES / 3, 16).unwrap();
        assert!(!arena.allocate(shared).is_null());
        assert!(!arena.allocate(shared).is_null());
        assert!(!arena.allocate(shared).is_null());

        with_snapshot_arena(|| {
            let address = snapshot_arena_allocate(Layout::from_size_align(64, 8).unwrap()).unwrap();
            with_snapshot_arena(|| assert!(snapshot_arena_deallocate(address)));
        });
    }

    #[test]
    fn fixed_buffer_reader_and_writer_reject_short_storage() {
        let mut byte = [0_u8; 1];
        assert!(Writer::new(&mut byte).u64(1).is_err());
        assert!(Reader::new(&[]).u64().is_err());
    }

    #[test]
    fn version_five_policies_and_legacy_thread_names_are_compatible() {
        let policy = RecordingPolicy {
            enabled: true,
            capture_backtraces: false,
            event_sampling: EventSampling::one_in(3).unwrap(),
        };
        let mut bytes = Vec::new();
        for value in [policy, policy, policy] {
            bytes.push(u8::from(value.enabled));
            bytes.push(u8::from(value.capture_backtraces));
            bytes.extend_from_slice(&0_u16.to_le_bytes());
            bytes.extend_from_slice(&u32::try_from(value.event_sampling.get()).unwrap().to_le_bytes());
        }
        let decoded = decode_recording_policies(&mut Reader::new(&bytes), 5).unwrap();
        assert_eq!(decoded.runtime_tasks, decoded.general_events);

        let mut thread = Vec::new();
        thread.extend_from_slice(&1_u64.to_le_bytes());
        thread.extend_from_slice(&2_u64.to_le_bytes());
        thread.extend_from_slice(&3_u64.to_le_bytes());
        assert_eq!(decode_threads(&mut Reader::new(&thread), 1, 1).unwrap()[0].name, "");
    }

    #[test]
    fn version_seven_policies_default_cache_recording_to_disabled() {
        let policy = RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            event_sampling: EventSampling::one_in(7).unwrap(),
        };
        let mut bytes = Vec::new();
        for value in [policy; 5] {
            bytes.push(u8::from(value.enabled));
            bytes.push(u8::from(value.capture_backtraces));
            bytes.extend_from_slice(&0_u16.to_le_bytes());
            bytes.extend_from_slice(&u32::try_from(value.event_sampling.get()).unwrap().to_le_bytes());
        }

        let decoded = decode_recording_policies(&mut Reader::new(&bytes), 7).unwrap();

        assert_eq!(decoded.cache, RecordingPolicy::default());
    }

    #[test]
    fn malformed_event_headers_are_rejected() {
        let mut current = Vec::new();
        current.extend_from_slice(&1_u64.to_le_bytes());
        current.push(EventKind::MutexAccess.wire_value());
        current.push(1);
        current.push(0);
        current.push(1);
        current.extend_from_slice(&[0; 64]);
        assert!(decode_event_v4(&mut Reader::new(&current)).is_err());

        let mut oversized_frames = current;
        oversized_frames[10] = u8::try_from(MAX_STACK_FRAMES + 1).unwrap();
        oversized_frames[11] = 0;
        assert!(decode_event_v4(&mut Reader::new(&oversized_frames)).is_err());

        let mut legacy = Vec::new();
        legacy.extend_from_slice(&1_u64.to_le_bytes());
        legacy.push(EventKind::MutexAccess.wire_value());
        legacy.push(0);
        legacy.extend_from_slice(&1_u16.to_le_bytes());
        assert!(decode_legacy_event(&mut Reader::new(&legacy), 1).is_err());
    }

    #[test]
    fn local_source_chains_detect_duplicates_and_capture_failures() {
        fn capture(_context: SnapshotContext<'_>) -> Result<SourceData, Error> {
            SourceData::copy_from(b"ok")
        }
        fn fail(_context: SnapshotContext<'_>) -> Result<SourceData, Error> {
            Err(Error::new("injected source failure"))
        }

        let first = Source::new(SourceId::new(101), "first", 1, capture);
        let second = Source::new(SourceId::new(101), "second", 1, capture);
        first.next.store(ptr::from_ref(&second).cast_mut(), Ordering::Relaxed);
        assert!(
            capture_sources_from(
                ptr::from_ref(&first).cast_mut(),
                SnapshotContext {
                    events: &Events::default()
                }
            )
            .is_err()
        );

        let failed = Source::new(SourceId::new(102), "failed", 1, fail);
        assert!(
            capture_sources_from(
                ptr::from_ref(&failed).cast_mut(),
                SnapshotContext {
                    events: &Events::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn arena_absence_and_large_layout_failures_are_reported() {
        let layout = Layout::from_size_align(8, 8).unwrap();
        assert_eq!(snapshot_arena_allocate(layout), None);
        assert!(!snapshot_arena_deallocate(NonNull::<u8>::dangling().as_ptr()));

        let mut arena = SnapshotArena::new();
        let huge = Layout::from_size_align(isize::MAX as usize, 1).unwrap();
        assert!(arena.allocate(huge).is_null());
        let unallocatable = Layout::from_size_align(1_usize << 60, 1).unwrap();
        assert!(arena.allocate(unallocatable).is_null());

        assert_eq!(snapshot_chunk_address(usize::MAX, 1, 1, 1, usize::MAX), None);
        assert_eq!(snapshot_chunk_address(usize::MAX - 1, 0, 1, 4, usize::MAX), None);
        assert_eq!(snapshot_chunk_address(0, 0, usize::MAX, 1, usize::MAX - 1), None);
        assert_eq!(snapshot_chunk_address(10, 0, 1, 1, 0), None);
    }
}
