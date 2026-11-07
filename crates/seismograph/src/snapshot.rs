// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Process snapshot capture, source registration, and encoding.
//!
//! Format version 8 adds independently configurable cache recording.
//! [`crate::snapshot::decode()`] continues to accept versions 1 through 7.
//! Unknown future format versions are rejected.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::{Duration, Instant};
use std::{fmt, ptr};

use crate::Error;
use crate::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind};
use crate::recorder::event::{
    Address, Event, EventClock, EventKind, EventPayload, EventSequence, EventTimestamp, Events, NumericEvent, ObjectId,
};
use crate::recorder::io::{BufferId, IoEvent, IoOperationId, IoOutcome, IoResourceId, IoResourceKind};
use crate::recorder::runtime::{RuntimeEvent, RuntimeId, WorkerId};
use crate::recorder::thread::{ThreadId, ThreadLog};
use crate::recorder::{self, EventSampling, MAX_EVENT_SAMPLING_ONE_IN, MAX_STACK_FRAMES, Policies, Policy, SuppressionGuard};
use crate::system::SystemSlice;

const MAGIC: [u8; 8] = *b"SEISMOG\0";
const FORMAT_VERSION: u16 = 8;
const HEADER_LEN: usize = 92;
const SOURCE_HEADER_LEN: usize = 24;
const EVENT_FIXED_LEN: usize = 92;
const LEGACY_EVENT_V1_FIXED_LEN: usize = 28;
const LEGACY_EVENT_V2_FIXED_LEN: usize = 76;
const THREAD_FIXED_LEN: usize = 24;
const THREAD_NAMED_FIXED_LEN: usize = 26;
static SOURCES: AtomicPtr<Source> = AtomicPtr::new(ptr::null_mut());

fn write_snapshot_count(value: usize, write: impl FnOnce(u32) -> Result<(), Error>) -> Result<(), Error> {
    write(snapshot_wire_count(value)?)
}

/// Treatment of recorder event buffers after a snapshot captures them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EventBufferDisposition {
    /// Keeps retained events and active-thread backing buffers.
    ///
    /// Buffers belonging to exited threads are released after capture.
    #[default]
    Retain,
    /// Discards retained events after capture while keeping active-thread buffers for reuse.
    Clear,
    /// Discards retained events after capture and releases active-thread buffers,
    /// then restores the previous recording policies (including enabled classes).
    Release,
    /// Captures events, disables all six recording classes, and releases their buffers.
    ///
    /// Unlike `Release`, recording is not resumed. Source or encoding failures
    /// after recorder cleanup do not undo the stop.
    /// A timeout before cleanup restores prior policies; a timeout during release
    /// leaves recording disabled and may leave some buffers allocated.
    Stop,
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
    /// Tries to create a source identity from its stable numeric representation.
    #[must_use]
    pub const fn from_raw(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

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
    capture: CaptureFn,
    registered: AtomicBool,
    next: AtomicPtr<Self>,
}

/// Callback that contributes one source payload to a snapshot.
pub type CaptureFn = for<'a> fn(SnapshotContext<'a>) -> Result<SourceData, Error>;

impl Source {
    /// Creates a source descriptor suitable for static storage.
    ///
    /// # Panics
    ///
    /// Panics when `name` is empty or `schema_version` is zero.
    #[must_use]
    pub const fn new(id: SourceId, name: &'static str, schema_version: u16, capture: CaptureFn) -> Self {
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
        .try_update(Ordering::Release, Ordering::Acquire, |head| {
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
        f.debug_struct("SourceData")
            .field("bytes", &self.bytes.as_slice().len())
            .finish_non_exhaustive()
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
    /// Time spent collecting recorder data and preparing source payloads.
    ///
    /// This partial duration ends before final container sizing, encoding and
    /// destruction of intermediate data. It excludes file I/O and any caller-side
    /// prewarming before collection starts; work inside source callbacks is included.
    /// It is not an end-to-end capture duration. Older format versions carry the
    /// same partial measurement.
    pub capture_duration_nanos: u64,
    /// Retained general-purpose runtime events.
    pub events: Events,
    /// Point-in-time data contributed by registered sources.
    pub sources: Vec<SourceSnapshot>,
}

/// An opaque encoded seismograph snapshot.
pub struct Snapshot {
    bytes: Vec<u8>,
}

impl Snapshot {
    fn zeroed(len: usize) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).map_err(|_error| Error::allocation_failed())?;
        bytes.resize(len, 0);
        Ok(Self { bytes })
    }

    /// Returns the complete encoded snapshot.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[cfg(any(test, feature = "monitor"))]
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
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
        f.debug_struct("Snapshot").field("bytes", &self.bytes.len()).finish_non_exhaustive()
    }
}

/// Captures general-purpose events and all registered snapshot sources.
///
/// The encoded duration measures collection and source preparation only, not final
/// container sizing/encoding, intermediate-data destruction, or subsequent file I/O.
/// Caller-side prewarming before collection starts is also excluded; source
/// callbacks executed during collection are included.
///
/// # Errors
///
/// Returns an error when a source fails, source identities conflict, or the
/// output buffer cannot be allocated.
pub(crate) fn snapshot(options: SnapshotOptions) -> Result<Snapshot, Error> {
    snapshot_with_recording(options, recorder::try_snapshot)
}

fn snapshot_with_recording(
    options: SnapshotOptions,
    recording: impl FnOnce(EventBufferDisposition) -> Result<Option<Events>, Error>,
) -> Result<Snapshot, Error> {
    let _suppression = SuppressionGuard::enter();
    let started_at = Instant::now();
    let decoded = prepare_snapshot_with(options, recording, || started_at.elapsed(), capture_sources)?;
    encode_snapshot(&decoded)
}

fn prepare_snapshot_with(
    options: SnapshotOptions,
    recording: impl FnOnce(EventBufferDisposition) -> Result<Option<Events>, Error>,
    elapsed: impl FnOnce() -> Duration,
    sources: impl FnOnce(SnapshotContext<'_>) -> Result<Vec<SourceSnapshot>, Error>,
) -> Result<DecodedSnapshot, Error> {
    let events = recording(options.event_buffers)?;
    let mut events = events.unwrap_or_default();
    events.clock = EventClock::CURRENT;
    let sources = sources(SnapshotContext { events: &events })?;
    Ok(DecodedSnapshot {
        capture_duration_nanos: snapshot_duration(elapsed),
        events,
        sources,
    })
}

fn snapshot_duration(elapsed: impl FnOnce() -> Duration) -> u64 {
    u64::try_from(elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Decodes a seismograph snapshot.
///
/// # Errors
///
/// Returns an error when the bytes are malformed or use an unsupported format.
pub fn decode(bytes: &[u8]) -> Result<DecodedSnapshot, Error> {
    decode_reader(&mut Reader::new(bytes))
}

fn decode_reader(reader: &mut Reader<'_>) -> Result<DecodedSnapshot, Error> {
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
    let recording = decode_recording_policies(reader, format_version)?;
    let clock = decode_clock(reader, format_version)?;

    validate_count_fits(thread_count, reader.remaining().len(), thread_fixed_len(format_version))?;
    let threads = decode_threads(reader, thread_count, format_version)?;

    validate_count_fits(event_count, reader.remaining().len(), event_fixed_len(format_version))?;
    let mut events = Vec::with_capacity(event_count);
    for _ in 0..event_count {
        let thread_id = ThreadId::new(reader.u64()?);
        let sequence = EventSequence::new(reader.u64()?);
        let (timestamp, kind, payload, frame_count) = if format_version >= 4 {
            decode_event_v4(reader)?
        } else {
            decode_legacy_event(reader, format_version)?
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

    validate_count_fits(source_count, reader.remaining().len(), SOURCE_HEADER_LEN)?;
    let mut sources = Vec::with_capacity(source_count);
    let mut source_ids = HashSet::with_capacity(source_count);
    for _ in 0..source_count {
        let id = SourceId::from_raw(reader.u64()?).ok_or_else(Error::invalid_format)?;
        if !source_ids.insert(id) {
            return Err(Error::invalid_format());
        }
        let schema_version = reader.u16()?;
        let name_len = reader.u16()? as usize;
        let data_len = reader.u64()?;
        // #[gamma::skip(expr.decrement, literal.int_decrement, literal.int_to_one, literal.int_to_zero, reason = "reducing the reserved name/header extent only weakens this admission check; the following checked reserved-field, name, and data reads reject the same truncated source with the same invalid-format error")]
        let data_len = snapshot_usize(data_len, reader.remaining().len().saturating_sub(4 + name_len))?;
        if reader.u32()? != 0 {
            return Err(Error::invalid_format());
        }
        let name = std::str::from_utf8(reader.read(name_len)?)
            .map_err(|_error| Error::invalid_format())?
            .to_owned();
        // #[gamma::skip(try.propagate_to_unwrap, reason = "snapshot_usize bounded data_len by the remainder after the reserved field and name; those preceding reads succeeded, so the data extent is available")]
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

    let total_events = threads.iter().try_fold(0_u64, |total, thread| {
        total.checked_add(thread.total_events).ok_or_else(Error::invalid_format)
    })?;
    let lost_events = threads.iter().try_fold(0_u64, |total, thread| {
        total.checked_add(thread.lost_events).ok_or_else(Error::invalid_format)
    })?;
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

// #[gamma::skip(fn_value.ok, parameter.default_shadow, tag = "outofmem", reason = "November 4 campaign: this mutation exceeded the memory limit")]
fn validate_count_fits(count: usize, remaining_bytes: usize, minimum_item_bytes: usize) -> Result<(), Error> {
    debug_assert!(minimum_item_bytes != 0);
    if
    // #[gamma::skip(cond.always_false, tag = "outofmem", reason = "November 4 campaign: this mutation exceeded the memory limit")]
    count > remaining_bytes / minimum_item_bytes {
        return
        // #[gamma::skip(result.err_to_ok, tag = "outofmem", reason = "November 4 campaign: this mutation exceeded the memory limit")]
        Err(Error::invalid_format());
    }
    Ok(())
}

const fn thread_fixed_len(format_version: u16) -> usize {
    if format_version >= 2 {
        THREAD_NAMED_FIXED_LEN
    } else {
        THREAD_FIXED_LEN
    }
}

const fn event_fixed_len(format_version: u16) -> usize {
    if format_version >= 4 {
        EVENT_FIXED_LEN
    } else if format_version >= 2 {
        LEGACY_EVENT_V2_FIXED_LEN
    } else {
        LEGACY_EVENT_V1_FIXED_LEN
    }
}

fn decode_sampling(reader: &mut Reader<'_>, format_version: u16) -> Result<u64, Error> {
    if format_version < 3 {
        return Ok(1);
    }
    let sampling = reader.u32()?;
    validated_sampling(u64::from(sampling)).map(u64::from)
}

fn decode_recording_policies(reader: &mut Reader<'_>, format_version: u16) -> Result<Policies, Error> {
    if format_version < 5 {
        let sampling = decode_sampling(reader, format_version)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "decode_sampling returns 1 for legacy data or a validated value at most 1048576, fitting usize on the supported 32/64-bit targets")]
        let sampling = usize::try_from(sampling).map_err(|_error| Error::invalid_format())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "decode_sampling already validated the identical nonzero EventSampling bound")]
        let sampling = EventSampling::one_in(sampling).ok_or_else(Error::invalid_format)?;
        let policy = Policy {
            enabled: true,
            capture_backtraces: false,
            event_sampling: sampling,
        };
        return Ok(Policies {
            allocations: policy,
            general_events: policy,
            arc_dereferences: policy,
            runtime_tasks: policy,
            io: Policy::default(),
            cache: Policy::default(),
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
        Policy::default()
    };
    let cache = if format_version >= 8 {
        decode_recording_policy(reader)?
    } else {
        Policy::default()
    };
    Ok(Policies {
        allocations,
        general_events,
        arc_dereferences,
        runtime_tasks,
        io,
        cache,
    })
}

fn decode_recording_policy(reader: &mut Reader<'_>) -> Result<Policy, Error> {
    let enabled = reader.u8()?;
    let capture_backtraces = reader.u8()?;
    if enabled > 1 || capture_backtraces > 1 || reader.u16()? != 0 {
        return Err(Error::invalid_format());
    }
    let sampling = reader.u32()?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "u32 fits usize on the supported 32/64-bit targets; raw sampling-policy validation remains outside the guard")]
    let sampling = usize::try_from(sampling).map_err(|_error| Error::invalid_format())?;
    let event_sampling = EventSampling::one_in(sampling).ok_or_else(Error::invalid_format)?;
    Ok(Policy {
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
    // #[gamma::skip(literal.int_increment, reason = "the following loop overwrites all eight fields before any decoded payload reads them")]
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
                // #[gamma::skip(try.propagate_to_unwrap, reason = "the zero branch was excluded and WorkerId::from_raw accepts every nonzero u64")]
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
        5 if fields[5] == 0 && {
            // #[gamma::skip(literal.int_decrement, reason = "IoOutcome accepts only wire values one through five; bit 47 would make that byte at least 128 and is independently rejected below")]
            fields[7] >> 48 == 0
        } =>
        {
            let metadata = fields[7];
            let buffer_id = match fields[2] {
                0 => None,
                // #[gamma::skip(try.propagate_to_unwrap, reason = "the zero arm was excluded and BufferId::from_raw accepts every nonzero u64")]
                value => Some(BufferId::from_raw(value).ok_or_else(Error::invalid_format)?),
            };
            Ok(EventPayload::Io(IoEvent {
                operation_id: IoOperationId::from_raw(fields[0]).ok_or_else(Error::invalid_format)?,
                resource_id: IoResourceId::from_raw(fields[1]).ok_or_else(Error::invalid_format)?,
                buffer_id,
                requested_bytes: fields[3],
                completed_bytes: fields[4],
                buffer_len: fields[6],
                // #[gamma::skip(try.propagate_to_unwrap, reason = "masking by u32::MAX proves the converted value fits u32")]
                buffer_span_count: u32::try_from(metadata & u64::from(u32::MAX)).map_err(|_error| Error::invalid_format())?,
                resource_kind: IoResourceKind::from_wire_value(
                    // #[gamma::skip(try.propagate_to_unwrap, reason = "masking by u8::MAX proves the converted value fits u8; invalid enum values remain separately rejected")]
                    u8::try_from((metadata >> 32) & u64::from(u8::MAX)).map_err(|_error| Error::invalid_format())?,
                )
                .ok_or_else(Error::invalid_format)?,
                outcome: IoOutcome::from_wire_value(
                    // #[gamma::skip(try.propagate_to_unwrap, reason = "masking by u8::MAX proves the converted value fits u8; invalid enum values remain separately rejected")]
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
    let value = snapshot_usize(value, MAX_EVENT_SAMPLING_ONE_IN)?;
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
        let data = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (descriptor.capture)(context)))
            .map_err(|_panic| Error::source_failed(descriptor.id, Error::new("seismograph source panicked during snapshot capture")))?
            .map_err(|error| Error::source_failed(descriptor.id, error))?;
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

fn snapshot_encoded_len(
    event_frame_counts: impl IntoIterator<Item = usize>,
    source_sizes: impl IntoIterator<Item = (usize, usize)>,
    thread_name_lengths: impl IntoIterator<Item = usize>,
) -> Result<usize, Error> {
    let events_len = event_frame_counts.into_iter().try_fold(0usize, |total, frame_count| {
        EVENT_FIXED_LEN
            .checked_add(frame_count.checked_mul(8)?)
            .and_then(|len| total.checked_add(len))
    });
    let sources_len = source_sizes.into_iter().try_fold(0usize, |total, (name_len, data_len)| {
        let len = SOURCE_HEADER_LEN.checked_add(name_len)?.checked_add(data_len)?;
        total.checked_add(len)
    });
    let threads_len = thread_name_lengths
        .into_iter()
        .try_fold(0usize, |total, name_len| total.checked_add(26)?.checked_add(name_len));
    HEADER_LEN
        .checked_add(threads_len.ok_or_else(Error::allocation_failed)?)
        .and_then(|len| len.checked_add(events_len?))
        .and_then(|len| len.checked_add(sources_len?))
        .ok_or_else(Error::allocation_failed)
}

fn snapshot_wire_count(value: usize) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_error| Error::invalid_format())
}

fn snapshot_usize(value: u64, maximum: usize) -> Result<usize, Error> {
    if value > maximum as u64 {
        return Err(Error::invalid_format());
    }
    usize::try_from(value).map_err(|_error| Error::invalid_format())
}

fn encode_snapshot(snapshot: &DecodedSnapshot) -> Result<Snapshot, Error> {
    encode_snapshot_with(
        snapshot,
        |snapshot| {
            snapshot_encoded_len(
                snapshot.events.events.iter().map(|event| event.call_stack.len()),
                snapshot.sources.iter().map(|source| (source.name.len(), source.data.len())),
                snapshot.events.threads.iter().map(|thread| thread.name.len()),
            )
        },
        Snapshot::zeroed,
    )
}

fn encode_snapshot_with(
    snapshot: &DecodedSnapshot,
    length: impl FnOnce(&DecodedSnapshot) -> Result<usize, Error>,
    allocate: impl FnOnce(usize) -> Result<Snapshot, Error>,
) -> Result<Snapshot, Error> {
    encode_snapshot_with_counts(snapshot, length, allocate, |writer, count| {
        write_snapshot_count(count, |count| writer.u32(count))
    })
}

fn encode_snapshot_with_counts(
    snapshot: &DecodedSnapshot,
    length: impl FnOnce(&DecodedSnapshot) -> Result<usize, Error>,
    allocate: impl FnOnce(usize) -> Result<Snapshot, Error>,
    mut write_count: impl FnMut(&mut Writer<'_>, usize) -> Result<(), Error>,
) -> Result<Snapshot, Error> {
    let len = length(snapshot)?;
    let mut encoded = allocate(len)?;
    let mut writer = Writer::new(&mut encoded.bytes);
    // #[gamma::skip(try.propagate_to_unwrap, reason = "snapshot_encoded_len and zeroed provide the complete exact output buffer before this fixed-size write")]
    writer.write(&MAGIC)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed version field")]
    writer.u16(FORMAT_VERSION)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed reserved field")]
    writer.u16(0)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed duration field")]
    writer.u64(snapshot.capture_duration_nanos)?;
    write_count(&mut writer, snapshot.events.threads.len())?;
    write_count(&mut writer, snapshot.events.events.len())?;
    write_count(&mut writer, snapshot.sources.len())?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "RecordingPolicy contains constructor-validated EventSampling at most 1048576 and the exact output length includes all eight policy bytes")]
    encode_recording_policy(&mut writer, snapshot.events.recording.allocations)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "RecordingPolicy contains constructor-validated EventSampling at most 1048576 and the exact output length includes all eight policy bytes")]
    encode_recording_policy(&mut writer, snapshot.events.recording.general_events)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "RecordingPolicy contains constructor-validated EventSampling at most 1048576 and the exact output length includes all eight policy bytes")]
    encode_recording_policy(&mut writer, snapshot.events.recording.arc_dereferences)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "RecordingPolicy contains constructor-validated EventSampling at most 1048576 and the exact output length includes all eight policy bytes")]
    encode_recording_policy(&mut writer, snapshot.events.recording.runtime_tasks)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "RecordingPolicy contains constructor-validated EventSampling at most 1048576 and the exact output length includes all eight policy bytes")]
    encode_recording_policy(&mut writer, snapshot.events.recording.io)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "RecordingPolicy contains constructor-validated EventSampling at most 1048576 and the exact output length includes all eight policy bytes")]
    encode_recording_policy(&mut writer, snapshot.events.recording.cache)?;
    // #[gamma::skip(cond.always_false, reason = "Unspecified is the only non-CURRENT clock and the following required ticks_per_second still rejects it with the same invalid-format error")]
    if snapshot.events.clock != EventClock::CURRENT {
        return Err(Error::invalid_format());
    }
    // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed clock field")]
    writer.u16(snapshot.events.clock.wire_value())?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed reserved field")]
    writer.u16(0)?;
    // #[gamma::skip(try.propagate_to_unwrap, reason = "the preceding CURRENT clock check proves a frequency exists, and the exact encoded length includes its fixed field")]
    writer.u64(snapshot.events.clock.ticks_per_second().ok_or_else(Error::invalid_format)?)?;
    for thread in &snapshot.events.threads {
        // #[gamma::skip(try.propagate_to_unwrap, reason = "snapshot_encoded_len includes each thread record and name before allocating the exact output buffer")]
        writer.u64(thread.thread_id.get())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes every thread counter")]
        writer.u64(thread.total_events)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes every thread counter")]
        writer.u64(thread.lost_events)?;
        let name_len = u16::try_from(thread.name.len()).map_err(|_error| Error::invalid_format())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed name-length field; the fallible u16 narrowing remains outside the guard")]
        writer.u16(name_len)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "snapshot_encoded_len includes this exact thread name byte length")]
        writer.write(thread.name.as_bytes())?;
    }
    for event in &snapshot.events.events {
        validate_payload(event.kind, event.payload)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "snapshot_encoded_len includes each fixed event record and its exact stack length")]
        writer.u64(event.thread_id.get())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed event sequence")]
        writer.u64(event.sequence.get())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed event timestamp")]
        writer.u64(event.timestamp.ticks())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed event kind")]
        writer.u8(event.kind.wire_value())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed payload tag")]
        writer.u8(payload_tag(event.payload))?;
        let stack_len = u8::try_from(event.call_stack.len()).map_err(|_error| Error::invalid_format())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed stack-length field; the fallible u8 narrowing remains outside the guard")]
        writer.u8(stack_len)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed reserved byte")]
        writer.u8(0)?;
        for field in encode_payload(event.payload) {
            // #[gamma::skip(try.propagate_to_unwrap, reason = "the fixed event length includes all eight payload fields")]
            writer.u64(field)?;
        }
        for &frame in &event.call_stack {
            // #[gamma::skip(try.propagate_to_unwrap, reason = "snapshot_encoded_len counted this exact stack length before output allocation")]
            writer.u64(frame.get())?;
        }
    }
    for source in &snapshot.sources {
        // #[gamma::skip(try.propagate_to_unwrap, reason = "snapshot_encoded_len includes each source header and exact name and data lengths")]
        writer.u64(source.id.get())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed source schema field")]
        writer.u16(source.schema_version)?;
        let name_len = u16::try_from(source.name.len()).map_err(|_error| Error::invalid_format())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed name-length field; the fallible u16 narrowing remains outside the guard")]
        writer.u16(name_len)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "usize fits u64 on supported targets and the exact encoded length includes this fixed source-data length field")]
        writer.u64(u64::try_from(source.data.len()).map_err(|_error| Error::invalid_format())?)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "the exact encoded length includes this fixed source reserved field")]
        writer.u32(0)?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "snapshot_encoded_len includes this exact source name length")]
        writer.write(source.name.as_bytes())?;
        // #[gamma::skip(try.propagate_to_unwrap, reason = "snapshot_encoded_len includes this exact source data length")]
        writer.write(&source.data)?;
    }
    debug_assert!(writer.remaining().is_empty(), "encoded length accounts for every written field");
    Ok(encoded)
}

fn encode_recording_policy(writer: &mut Writer<'_>, policy: Policy) -> Result<(), Error> {
    writer.u8(u8::from(policy.enabled))?;
    writer.u8(u8::from(policy.capture_backtraces))?;
    writer.u16(0)?;
    let sampling = u64::try_from(policy.event_sampling.get()).unwrap_or(
        // #[gamma::skip(expr.decrement, reason = "EventSampling::one_in caps this private-field type at 1048576, which always fits u64")]
        u64::MAX,
    );
    // #[gamma::skip(try.propagate_to_unwrap, reason = "EventSampling::one_in guarantees 1 through 1048576, so validated_sampling cannot fail; the writer operation is outside this guard")]
    let sampling = validated_sampling(sampling)?;
    writer.u32(sampling)
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
            encode_heap_kind(allocation.heap_kind) + if allocation.freed_after_heap_release { 1 << 8 } else { 0 },
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
            io.buffer_span_count as u64 + ((io.resource_kind.wire_value() as u64) << 32) + ((io.outcome.wire_value() as u64) << 40),
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
    bytes: SystemSlice<u8>,
}

impl SystemBytes {
    fn zeroed(len: usize) -> Option<Self> {
        SystemSlice::try_zeroed(len).map(|bytes| Self { bytes })
    }

    fn copy_from(bytes: &[u8]) -> Option<Self> {
        Self::copy_from_with(bytes, Self::zeroed)
    }

    fn copy_from_with(bytes: &[u8], allocate: impl FnOnce(usize) -> Option<Self>) -> Option<Self> {
        let mut result = allocate(bytes.len())?;
        result.as_mut_slice().copy_from_slice(bytes);
        Some(result)
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
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
    #[cfg(test)]
    requested_bytes: usize,
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            remaining: bytes,
            #[cfg(test)]
            requested_bytes: 0,
        }
    }

    const fn remaining(&self) -> &'a [u8] {
        self.remaining
    }

    fn read(&mut self, len: usize) -> Result<&'a [u8], Error> {
        #[cfg(test)]
        {
            self.requested_bytes = self.requested_bytes.saturating_add(len);
        }
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
    use crate::recorder::{Policies as RecordingPolicies, Policy as RecordingPolicy};

    #[test]
    fn nov6_encoder_returns_each_header_count_error_before_writing_later_fields() {
        for rejected in 0..3 {
            let mut calls = 0;
            let error = encode_snapshot_with_counts(
                &DecodedSnapshot::default(),
                |_| Ok(HEADER_LEN),
                Snapshot::zeroed,
                |writer, value| {
                    assert_eq!(value, 0);
                    let index = calls;
                    calls += 1;
                    if index == rejected {
                        Err(Error::new("count destination refused"))
                    } else {
                        writer.u32(0)
                    }
                },
            )
            .unwrap_err();
            assert_eq!(error.to_string(), "count destination refused");
            assert_eq!(calls, rejected + 1);
        }
    }

    #[test]
    fn nov6_sampling_admission_accepts_its_exact_limit_and_returns_errors_above_it() {
        assert_eq!(
            validated_sampling(MAX_EVENT_SAMPLING_ONE_IN as u64).unwrap(),
            u32::try_from(MAX_EVENT_SAMPLING_ONE_IN).unwrap()
        );
        for invalid in [0, MAX_EVENT_SAMPLING_ONE_IN as u64 + 1, usize::MAX as u64, u64::MAX] {
            assert_eq!(
                validated_sampling(invalid).unwrap_err().to_string(),
                Error::invalid_format().to_string()
            );
        }
    }

    #[test]
    fn november_snapshot_preparation_preserves_injected_duration_and_source_context_without_clock_or_global_state() {
        for (duration, expected) in [(Duration::ZERO, 0), (Duration::from_nanos(37), 37), (Duration::MAX, u64::MAX)] {
            let calls = std::cell::RefCell::new(Vec::new());
            let decoded = prepare_snapshot_with(
                SnapshotOptions {
                    event_buffers: EventBufferDisposition::Clear,
                },
                |disposition| {
                    assert_eq!(disposition, EventBufferDisposition::Clear);
                    calls.borrow_mut().push("record");
                    Ok(None)
                },
                || {
                    assert_eq!(*calls.borrow(), ["record", "sources"]);
                    calls.borrow_mut().push("elapsed");
                    duration
                },
                |context| {
                    assert_eq!(*calls.borrow(), ["record"]);
                    assert_eq!(context.events.clock, EventClock::CURRENT);
                    assert!(context.events.threads.is_empty());
                    assert!(context.events.events.is_empty());
                    calls.borrow_mut().push("sources");
                    Ok(Vec::new())
                },
            )
            .unwrap();
            assert_eq!(decoded.capture_duration_nanos, expected);
            assert_eq!(decoded.events.clock, EventClock::CURRENT);
            assert!(decoded.sources.is_empty());
            assert_eq!(*calls.borrow(), ["record", "sources", "elapsed"]);
        }
    }

    #[test]
    fn november_snapshot_preparation_propagates_recording_and_source_errors_before_duration_observation() {
        for fail_recording in [false, true] {
            let error = prepare_snapshot_with(
                SnapshotOptions {
                    event_buffers: EventBufferDisposition::Clear,
                },
                |_| {
                    if fail_recording {
                        Err(Error::new("recording refused"))
                    } else {
                        Ok(None)
                    }
                },
                || panic!("failed preparation must not observe duration"),
                |_| {
                    assert!(!fail_recording);
                    Err(Error::new("source refused"))
                },
            )
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                if fail_recording { "recording refused" } else { "source refused" }
            );
        }
    }

    #[test]
    fn november_snapshot_count_writes_preserve_admission_and_destination_errors_without_large_vectors() {
        for count in [0_u32, 1, 0x1234_5678, u32::MAX] {
            let mut observed = None;
            write_snapshot_count(count as usize, |value| {
                observed = Some(value);
                Ok(())
            })
            .unwrap();
            assert_eq!(observed, Some(count));
            let error = write_snapshot_count(count as usize, |_| Err(Error::new("writer refused"))).unwrap_err();
            assert_eq!(error.to_string(), "writer refused");
        }
        #[cfg(target_pointer_width = "64")]
        assert_eq!(
            write_snapshot_count(u32::MAX as usize + 1, |_| panic!("unrepresentable count must not reach writer"))
                .unwrap_err()
                .to_string(),
            Error::invalid_format().to_string()
        );
    }

    #[test]
    fn november_snapshot_encoding_preserves_sizing_and_allocator_errors_before_writing() {
        let snapshot = DecodedSnapshot::default();
        let error = encode_snapshot_with(
            &snapshot,
            |actual| {
                assert!(ptr::eq(actual, &raw const snapshot));
                Err(Error::new("size overflow"))
            },
            |_| panic!("size failure must not allocate"),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "size overflow");
        let error = encode_snapshot_with(
            &snapshot,
            |_| Ok(137),
            |length| {
                assert_eq!(length, 137);
                Err(Error::new("allocator refused"))
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "allocator refused");
    }

    #[test]
    fn november_snapshot_platform_lengths_model_32_bit_admission_without_allocating_or_truncating() {
        for value in [0, 1, u64::from(u32::MAX)] {
            assert_eq!(snapshot_usize(value, u32::MAX as usize).unwrap() as u64, value);
        }
        for value in [u64::from(u32::MAX) + 1, u64::MAX] {
            assert_eq!(
                snapshot_usize(value, u32::MAX as usize).unwrap_err().to_string(),
                Error::invalid_format().to_string()
            );
        }
        assert_eq!(snapshot_usize(usize::MAX as u64, usize::MAX).unwrap(), usize::MAX);
        assert_eq!(snapshot_usize(8, 7).unwrap_err().to_string(), Error::invalid_format().to_string());
        assert_eq!(snapshot_usize(7, 7).unwrap(), 7);
    }

    #[test]
    fn november_system_byte_copy_propagates_allocation_failure_and_copies_exact_payload_after_success() {
        assert!(
            SystemBytes::copy_from_with(b"ABC", |length| {
                assert_eq!(length, 3);
                None
            })
            .is_none()
        );
        let copied = SystemBytes::copy_from_with(b"ABC", |length| {
            assert_eq!(length, 3);
            SystemBytes::zeroed(length)
        })
        .unwrap();
        assert_eq!(copied.as_slice(), b"ABC");
    }

    fn snapshot_count_header(version: u16) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u64.to_le_bytes());
        bytes.extend_from_slice(&[0; 12]);
        if version >= 5 {
            let policies = match version {
                5 => 3,
                6 => 4,
                7 => 5,
                _ => 6,
            };
            for _ in 0..policies {
                bytes.extend_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0]);
            }
        } else if version >= 3 {
            bytes.extend_from_slice(&1_u32.to_le_bytes());
        }
        if version >= 4 {
            bytes.extend_from_slice(&EventClock::ProcessMonotonic.wire_value().to_le_bytes());
            bytes.extend_from_slice(&0_u16.to_le_bytes());
            bytes.extend_from_slice(&1_000_000_000_u64.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn november_snapshot_count_admission_uses_exact_versioned_lengths_before_any_row_reads() {
        for (version, header_len, thread_len, event_len) in [
            (1, 32, 24, 28),
            (2, 32, 26, 76),
            (3, 36, 26, 76),
            (4, 48, 26, 92),
            (5, 68, 26, 92),
            (6, 76, 26, 92),
            (7, 84, 26, 92),
            (8, 92, 26, 92),
        ] {
            for (offset, record_len) in [(20, thread_len), (24, event_len), (28, 24)] {
                let mut bytes = snapshot_count_header(version);
                assert_eq!(bytes.len(), header_len);
                bytes[offset..offset + 4].copy_from_slice(&1_u32.to_le_bytes());
                bytes.resize(header_len + record_len - 1, 0);
                let mut reader = Reader::new(&bytes);
                assert!(matches!(
                    decode_reader(&mut reader).unwrap_err().kind,
                    crate::ErrorKind::InvalidFormat
                ));
                assert_eq!(reader.requested_bytes, header_len, "version {version}, count offset {offset}");
                assert_eq!(reader.remaining().len(), record_len - 1);
                bytes.push(0);
                let mut reader = Reader::new(&bytes);
                let _ = decode_reader(&mut reader);
                assert!(
                    reader.requested_bytes > header_len,
                    "an exactly fitting row must pass count admission"
                );
            }
        }
    }

    #[test]
    fn gamma_round2_policy_writer_returns_short_buffer_errors_at_each_field() {
        for length in 0..8 {
            let mut output = vec![0xa5; length];
            assert!(matches!(
                encode_recording_policy(&mut Writer::new(&mut output), RecordingPolicy::all(true))
                    .unwrap_err()
                    .kind,
                crate::ErrorKind::InvalidFormat
            ));
        }
    }

    fn gamma_payloads() -> [(EventKind, EventPayload, u8, [u64; 8]); 5] {
        [
            (
                EventKind::MutexAccess,
                EventPayload::Object(ObjectId::new(13)),
                1,
                [13, 0, 0, 0, 0, 0, 0, 0],
            ),
            (
                EventKind::ChannelHighWatermark,
                EventPayload::Numeric(NumericEvent {
                    object_id: ObjectId::new(13),
                    value: 17,
                }),
                2,
                [13, 17, 0, 0, 0, 0, 0, 0],
            ),
            (
                EventKind::Allocation,
                EventPayload::Allocation(Allocation {
                    allocation_id: AllocationId::new(13),
                    event_thread_id: EventThreadId::new(17),
                    heap_id: HeapId::new(19),
                    heap_kind: HeapKind::Bump,
                    freed_after_heap_release: true,
                    address: Address::new(23),
                    size: 29,
                    alignment: 32,
                }),
                3,
                [13, 17, 19, 23, 29, 32, 258, 0],
            ),
            (
                EventKind::TaskSpawned,
                EventPayload::Runtime(RuntimeEvent {
                    runtime_id: RuntimeId::from_raw(13).unwrap(),
                    worker_id: Some(WorkerId::from_raw(17).unwrap()),
                    subject_id: 19,
                    related_id: 23,
                    value_0: 29,
                    value_1: 31,
                }),
                4,
                [13, 17, 19, 23, 29, 31, 0, 0],
            ),
            (
                EventKind::IoWriteFinished,
                EventPayload::Io(IoEvent {
                    operation_id: IoOperationId::from_raw(13).unwrap(),
                    resource_id: IoResourceId::from_raw(17).unwrap(),
                    buffer_id: Some(BufferId::from_raw(19).unwrap()),
                    requested_bytes: 23,
                    completed_bytes: 29,
                    buffer_len: 31,
                    buffer_span_count: 37,
                    resource_kind: IoResourceKind::File,
                    outcome: IoOutcome::Success,
                }),
                5,
                [13, 17, 19, 23, 29, 0, 31, 0x25 | (1 << 32) | (2 << 40)],
            ),
        ]
    }

    #[test]
    fn gamma_survivor_source_registry_keeps_links_and_registration_is_idempotent() {
        fn capture(_: SnapshotContext<'_>) -> Result<SourceData, Error> {
            SourceData::copy_from(b"data")
        }
        static FIRST: Source = Source::new(SourceId::new(0x0067_616d_6d61), "gamma-first", 1, capture);
        static SECOND: Source = Source::new(SourceId::new(0x0067_616d_6d62), "gamma-second", 1, capture);
        struct Restore(*mut Source);
        impl Drop for Restore {
            fn drop(&mut self) {
                SOURCES.store(self.0, Ordering::Release);
                for source in [&FIRST, &SECOND] {
                    source.registered.store(false, Ordering::Release);
                    source.next.store(ptr::null_mut(), Ordering::Release);
                }
            }
        }
        let _test = recorder::TEST_LOCK.lock().unwrap();
        let old = SOURCES.load(Ordering::Acquire);
        let _restore = Restore(old);
        register_source(&FIRST);
        assert_eq!(FIRST.next.load(Ordering::Acquire), old);
        register_source(&SECOND);
        assert_eq!(SECOND.next.load(Ordering::Acquire), ptr::from_ref(&FIRST).cast_mut());
        register_source(&SECOND);
        assert_eq!(SOURCES.load(Ordering::Acquire), ptr::from_ref(&SECOND).cast_mut());
        assert_eq!(SECOND.next.load(Ordering::Acquire), ptr::from_ref(&FIRST).cast_mut());
    }

    #[test]
    fn gamma_survivor_source_panic_preserves_exact_failure_context() {
        fn capture(_: SnapshotContext<'_>) -> Result<SourceData, Error> {
            panic!("scripted capture failure");
        }
        let mut source = Source::new(SourceId::new(13), "gamma-panics", 1, capture);
        let events = Events::default();
        let error = capture_sources_from(ptr::from_mut(&mut source), SnapshotContext { events: &events }).unwrap_err();
        assert_eq!(
            error.to_string(),
            "seismograph source 13 failed: seismograph source panicked during snapshot capture"
        );
    }

    #[test]
    fn gamma_survivor_encoder_rejects_representable_invalid_models() {
        let model = DecodedSnapshot {
            events: Events {
                clock: EventClock::CURRENT,
                ..Events::default()
            },
            ..DecodedSnapshot::default()
        };
        let mut invalid = model.clone();
        invalid.events.clock = EventClock::Unspecified;
        assert!(encode_snapshot(&invalid).is_err());
        let mut invalid = model.clone();
        invalid.events.threads.push(ThreadLog {
            thread_id: ThreadId::new(1),
            name: "x".repeat(65536),
            total_events: 0,
            lost_events: 0,
        });
        assert!(encode_snapshot(&invalid).is_err());
        let mut invalid = model.clone();
        invalid.sources.push(SourceSnapshot {
            id: SourceId::new(1),
            name: "x".repeat(65536),
            schema_version: 1,
            data: Vec::new(),
        });
        assert!(encode_snapshot(&invalid).is_err());
        let mut invalid = model;
        invalid.events.events.push(Event {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(1),
            timestamp: EventTimestamp::from_ticks(1),
            kind: EventKind::MutexAccess,
            payload: EventPayload::Numeric(NumericEvent {
                object_id: ObjectId::new(1),
                value: 2,
            }),
            call_stack: Vec::new(),
        });
        assert!(encode_snapshot(&invalid).is_err());
        invalid.events.events[0].payload = EventPayload::Object(ObjectId::new(1));
        invalid.events.events[0].call_stack = vec![Address::new(2); 256];
        assert!(encode_snapshot(&invalid).is_err());
    }

    #[test]
    fn gamma_survivor_encoder_matches_independent_full_wire_image() {
        let policies = std::array::from_fn::<_, 6, _>(|index| RecordingPolicy {
            enabled: index % 2 == 0,
            capture_backtraces: index % 2 != 0,
            event_sampling: EventSampling::one_in(index + 3).unwrap(),
        });
        let model = DecodedSnapshot {
            capture_duration_nanos: 0x0807_0605_0403_0201,
            events: Events {
                clock: EventClock::CURRENT,
                total_events: 29,
                lost_events: 11,
                recording: RecordingPolicies {
                    allocations: policies[0],
                    general_events: policies[1],
                    arc_dereferences: policies[2],
                    runtime_tasks: policies[3],
                    io: policies[4],
                    cache: policies[5],
                },
                threads: vec![ThreadLog {
                    thread_id: ThreadId::new(0x102),
                    total_events: 29,
                    lost_events: 11,
                    name: "λab".into(),
                }],
                events: gamma_payloads()
                    .into_iter()
                    .map(|(kind, payload, _, _)| Event {
                        thread_id: ThreadId::new(0x102),
                        sequence: EventSequence::new(0x304),
                        timestamp: EventTimestamp::from_ticks(0x506),
                        kind,
                        payload,
                        call_stack: vec![Address::new(0x708), Address::new(0x90a)],
                    })
                    .collect(),
            },
            sources: vec![SourceSnapshot {
                id: SourceId::new(17),
                name: "α".into(),
                schema_version: 0x201,
                data: vec![1, 3, 5],
            }],
        };
        let mut expected = b"SEISMOG\0".to_vec();
        expected.extend_from_slice(&[8, 0, 0, 0]);
        expected.extend_from_slice(&0x0807_0605_0403_0201_u64.to_le_bytes());
        for count in [1_u32, 5, 1] {
            expected.extend_from_slice(&count.to_le_bytes());
        }
        for index in 0_u32..6 {
            expected.extend_from_slice(&[u8::from(index % 2 == 0), u8::from(index % 2 != 0), 0, 0]);
            expected.extend_from_slice(&(index + 3).to_le_bytes());
        }
        expected.extend_from_slice(&[1, 0, 0, 0]);
        expected.extend_from_slice(&1_000_000_000_u64.to_le_bytes());
        for value in [0x102_u64, 29, 11] {
            expected.extend_from_slice(&value.to_le_bytes());
        }
        expected.extend_from_slice(&[4, 0]);
        expected.extend_from_slice("λab".as_bytes());
        for (kind, _, tag, fields) in gamma_payloads() {
            for value in [0x102_u64, 0x304, 0x506] {
                expected.extend_from_slice(&value.to_le_bytes());
            }
            expected.extend_from_slice(&[kind.wire_value(), tag, 2, 0]);
            for value in fields.into_iter().chain([0x708, 0x90a]) {
                expected.extend_from_slice(&value.to_le_bytes());
            }
        }
        expected.extend_from_slice(&17_u64.to_le_bytes());
        expected.extend_from_slice(&[1, 2, 2, 0]);
        expected.extend_from_slice(&3_u64.to_le_bytes());
        expected.extend_from_slice(&[0; 4]);
        expected.extend_from_slice("α".as_bytes());
        expected.extend_from_slice(&[1, 3, 5]);
        let encoded = encode_snapshot(&model).unwrap();
        assert_eq!(encoded.as_bytes(), expected);
        assert_eq!(decode(&expected).unwrap(), model);
        assert_eq!(format!("{encoded:?}"), format!("Snapshot {{ bytes: {}, .. }}", expected.len()));
    }

    #[test]
    fn snapshot_storage_is_zeroed_and_moves_without_copying() {
        let snapshot = Snapshot::zeroed(32).unwrap();
        let address = snapshot.as_bytes().as_ptr();
        let bytes = snapshot.into_bytes();

        assert_eq!((bytes.as_ptr(), bytes.as_slice()), (address, [0; 32].as_slice()));
        assert_eq!(Snapshot::zeroed(0).unwrap().as_bytes(), &[]);
    }

    #[test]
    fn snapshot_storage_reports_unrepresentable_capacity() {
        Snapshot::zeroed(usize::MAX).unwrap_err();
    }

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
        register_source(&TEST_SOURCE);
        recorder::configure(recorder::Configuration {
            arc_dereferences: recorder::Policy {
                enabled: true,
                ..Default::default()
            },
            ..recorder::test_configuration()
        });
        crate::record(recorder::event::EventClass::ArcDereference, || {
            Some(recorder::event::Record::object(EventKind::ArcDeref, ObjectId::new(42)))
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
        let recording = recorder::Policies {
            allocations: recorder::Policy {
                enabled: true,
                capture_backtraces: true,
                event_sampling: recorder::EventSampling::one_in(16).expect("valid test sampling"),
            },
            general_events: recorder::Policy {
                enabled: false,
                capture_backtraces: true,
                event_sampling: recorder::EventSampling::one_in(4).expect("valid test sampling"),
            },
            arc_dereferences: recorder::Policy {
                enabled: true,
                capture_backtraces: false,
                event_sampling: recorder::EventSampling::one_in(100).expect("valid test sampling"),
            },
            runtime_tasks: recorder::Policy {
                enabled: true,
                capture_backtraces: true,
                event_sampling: recorder::EventSampling::one_in(8).expect("valid test sampling"),
            },
            io: recorder::Policy {
                enabled: true,
                capture_backtraces: false,
                event_sampling: recorder::EventSampling::one_in(2).expect("valid test sampling"),
            },
            cache: recorder::Policy {
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
                worker_id: None,
                subject_id: 3,
                related_id: 4,
                value_0: 5,
                value_1: 6,
            }),
            call_stack: vec![Address::new(0x1234)],
        };
        let ready = Event {
            sequence: EventSequence::new(10),
            kind: EventKind::TaskReady,
            payload: EventPayload::Runtime(RuntimeEvent {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: None,
                subject_id: 3,
                related_id: 0,
                value_0: 0,
                value_1: 0,
            }),
            call_stack: Vec::new(),
            ..event
        };
        let snapshot = encode_snapshot(&DecodedSnapshot {
            capture_duration_nanos: 1,
            events: Events {
                clock: EventClock::CURRENT,
                total_events: 2,
                lost_events: 0,
                recording: Policies::default(),
                threads: vec![ThreadLog {
                    thread_id: ThreadId::new(7),
                    total_events: 2,
                    lost_events: 0,
                    name: "worker".to_owned(),
                }],
                events: vec![event.clone(), ready.clone()],
            },
            sources: Vec::new(),
        })
        .unwrap();

        assert_eq!(decode(snapshot.as_bytes()).unwrap().events.events, vec![event, ready]);
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
                recording: Policies::default(),
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
    fn source_storage_debug_and_file_persistence_work() {
        fn capture(_context: SnapshotContext<'_>) -> Result<SourceData, Error> {
            SourceData::copy_from(b"source")
        }

        let source = Source::new(SourceId::new(99), "local", 1, capture);
        assert!(format!("{source:?}").contains("Source"));
        assert_eq!(
            (source.capture)(SnapshotContext {
                events: &Events::default(),
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
        #[cfg(not(miri))]
        {
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
                    buffer_id: None,
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
    fn optional_runtime_and_io_identities_encode_and_decode() {
        let runtime = EventPayload::Runtime(RuntimeEvent {
            runtime_id: RuntimeId::from_raw(1).unwrap(),
            worker_id: Some(WorkerId::from_raw(2).unwrap()),
            subject_id: 3,
            related_id: 4,
            value_0: 5,
            value_1: 6,
        });
        let io = EventPayload::Io(IoEvent {
            operation_id: IoOperationId::from_raw(7).unwrap(),
            resource_id: IoResourceId::from_raw(8).unwrap(),
            buffer_id: Some(BufferId::from_raw(9).unwrap()),
            requested_bytes: 10,
            completed_bytes: 11,
            buffer_len: 12,
            buffer_span_count: 13,
            resource_kind: IoResourceKind::File,
            outcome: IoOutcome::Pending,
        });

        assert_eq!(
            (
                decode_payload(4, encode_payload(runtime)).unwrap(),
                decode_payload(5, encode_payload(io)).unwrap(),
            ),
            (runtime, io)
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one malformed-input matrix covers the complete snapshot framing contract"
    )]
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

        let duplicate_sources = encode_snapshot(&DecodedSnapshot {
            capture_duration_nanos: 0,
            events: Events {
                clock: EventClock::CURRENT,
                ..Events::default()
            },
            sources: vec![
                SourceSnapshot {
                    id: SourceId::from_raw(1).unwrap(),
                    name: "first".into(),
                    schema_version: 1,
                    data: vec![1],
                },
                SourceSnapshot {
                    id: SourceId::from_raw(1).unwrap(),
                    name: "second".into(),
                    schema_version: 2,
                    data: vec![2],
                },
            ],
        })
        .unwrap();
        assert!(decode(duplicate_sources.as_bytes()).is_err());

        let mut invalid_source = valid.as_bytes().to_vec();
        invalid_source[28..32].copy_from_slice(&1_u32.to_le_bytes());
        invalid_source.extend_from_slice(&[0; SOURCE_HEADER_LEN]);
        assert!(decode(&invalid_source).is_err());
        invalid_source[HEADER_LEN..HEADER_LEN + 8].copy_from_slice(&1_u64.to_le_bytes());
        invalid_source[HEADER_LEN + 20] = 1;
        assert!(decode(&invalid_source).is_err());

        for (total_events, lost_events) in [(u64::MAX, 0), (0, u64::MAX)] {
            let overflowing = encode_snapshot(&DecodedSnapshot {
                capture_duration_nanos: 0,
                events: Events {
                    clock: EventClock::CURRENT,
                    threads: vec![
                        ThreadLog {
                            thread_id: ThreadId::new(1),
                            total_events,
                            lost_events,
                            name: String::new(),
                        },
                        ThreadLog {
                            thread_id: ThreadId::new(2),
                            total_events: u64::from(total_events != 0),
                            lost_events: u64::from(lost_events != 0),
                            name: String::new(),
                        },
                    ],
                    ..Events::default()
                },
                sources: Vec::new(),
            })
            .unwrap();
            assert!(decode(overflowing.as_bytes()).is_err());
        }

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

        let invalid_clock_metadata = [1, 0, 1, 0, 0, 202, 154, 59, 0, 0, 0, 0];
        assert!(decode_clock(&mut Reader::new(&invalid_clock_metadata), FORMAT_VERSION).is_err());
    }

    #[test]
    fn declared_collection_counts_must_fit_in_remaining_bytes() {
        let snapshot = encode_snapshot(&DecodedSnapshot {
            capture_duration_nanos: 0,
            events: Events {
                clock: EventClock::CURRENT,
                ..Events::default()
            },
            sources: Vec::new(),
        })
        .unwrap();

        for offset in [20, 24, 28] {
            let mut bytes = snapshot.as_bytes().to_vec();
            bytes[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(decode(&bytes).is_err());
        }

        assert!(validate_count_fits(3, 24, 8).is_ok());
        assert!(validate_count_fits(4, 24, 8).is_err());
        assert_eq!(
            (
                thread_fixed_len(1),
                thread_fixed_len(2),
                event_fixed_len(1),
                event_fixed_len(2),
                event_fixed_len(4),
            ),
            (
                THREAD_FIXED_LEN,
                THREAD_NAMED_FIXED_LEN,
                LEGACY_EVENT_V1_FIXED_LEN,
                LEGACY_EVENT_V2_FIXED_LEN,
                EVENT_FIXED_LEN,
            )
        );
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

        for index in 0..6 {
            let mut fields = [0; 6];
            fields[index] = 1;
            assert!(
                decode_legacy_event(&mut Reader::new(&legacy_event(EventKind::MutexAccess, fields)), 3).is_err(),
                "legacy object field {index} must be reserved"
            );
        }
        for index in 1..6 {
            let mut fields = [0; 6];
            fields[index] = 1;
            assert!(
                decode_legacy_event(&mut Reader::new(&legacy_event(EventKind::ChannelHighWatermark, fields)), 3,).is_err(),
                "legacy numeric field {index} must be reserved"
            );
        }

        let not_released = decode_legacy_event(&mut Reader::new(&legacy_event(EventKind::Allocation, [2, 3, 4, 5, 8, 2])), 3).unwrap();
        assert!(matches!(
            not_released.2,
            EventPayload::Allocation(Allocation {
                heap_kind: HeapKind::Bump,
                freed_after_heap_release: false,
                ..
            })
        ));
    }

    #[test]
    fn policy_clock_and_event_boundaries_accept_only_exact_wire_shapes() {
        for bytes in [[2, 0, 0, 0, 1, 0, 0, 0], [0, 2, 0, 0, 1, 0, 0, 0], [0, 0, 1, 0, 1, 0, 0, 0]] {
            assert!(decode_recording_policy(&mut Reader::new(&bytes)).is_err());
        }
        assert_eq!(decode_clock(&mut Reader::new(&[]), 3).unwrap(), EventClock::Unspecified);

        let mut clock = Vec::new();
        clock.extend_from_slice(&EventClock::CURRENT.wire_value().to_le_bytes());
        clock.extend_from_slice(&0_u16.to_le_bytes());
        clock.extend_from_slice(&1_000_000_000_u64.to_le_bytes());
        assert_eq!(decode_clock(&mut Reader::new(&clock), 4).unwrap(), EventClock::CURRENT);

        let mut event = Vec::new();
        event.extend_from_slice(&1_u64.to_le_bytes());
        event.push(EventKind::MutexAccess.wire_value());
        event.push(1);
        event.push(u8::try_from(MAX_STACK_FRAMES).unwrap());
        event.push(0);
        event.extend_from_slice(&1_u64.to_le_bytes());
        event.extend_from_slice(&[0; 7 * 8]);
        assert_eq!(decode_event_v4(&mut Reader::new(&event)).unwrap().3, MAX_STACK_FRAMES);

        let mut legacy = Vec::new();
        legacy.extend_from_slice(&1_u64.to_le_bytes());
        legacy.push(EventKind::MutexAccess.wire_value());
        legacy.push(u8::try_from(MAX_STACK_FRAMES).unwrap());
        legacy.extend_from_slice(&0_u16.to_le_bytes());
        assert_eq!(decode_legacy_event(&mut Reader::new(&legacy), 1).unwrap().3, MAX_STACK_FRAMES);
    }

    #[test]
    fn payload_reserved_fields_and_packed_metadata_are_enforced() {
        for index in 1..8 {
            let mut fields = [0; 8];
            fields[0] = 1;
            fields[index] = 1;
            assert!(decode_payload(1, fields).is_err(), "object field {index} must be reserved");
        }
        for index in 2..8 {
            let mut fields = [0; 8];
            fields[0] = 1;
            fields[index] = 1;
            assert!(decode_payload(2, fields).is_err(), "numeric field {index} must be reserved");
        }
        let mut allocation = [1, 2, 3, 4, 5, 8, 1, 0];
        allocation[7] = 1;
        assert!(decode_payload(3, allocation).is_err());

        let mut runtime = [1, 0, 2, 3, 4, 5, 0, 0];
        runtime[6] = 1;
        assert!(decode_payload(4, runtime).is_err());
        runtime[6] = 0;
        runtime[7] = 1;
        assert!(decode_payload(4, runtime).is_err());

        let io = EventPayload::Io(IoEvent {
            operation_id: IoOperationId::from_raw(1).unwrap(),
            resource_id: IoResourceId::from_raw(2).unwrap(),
            buffer_id: Some(BufferId::from_raw(3).unwrap()),
            requested_bytes: 4,
            completed_bytes: 5,
            buffer_len: 6,
            buffer_span_count: 0x1122_3344,
            resource_kind: IoResourceKind::NamedPipe,
            outcome: IoOutcome::Canceled,
        });
        let encoded = encode_payload(io);
        assert_eq!(encoded[7], 0x0000_0404_1122_3344);
        assert_eq!(decode_payload(5, encoded).unwrap(), io);
        let mut reserved = encoded;
        reserved[5] = 1;
        assert!(decode_payload(5, reserved).is_err());
        let mut high_bits = encoded;
        high_bits[7] |= 1 << 48;
        assert!(decode_payload(5, high_bits).is_err());

        let allocation = EventPayload::Allocation(Allocation {
            allocation_id: AllocationId::new(1),
            event_thread_id: EventThreadId::new(2),
            heap_id: HeapId::new(3),
            heap_kind: HeapKind::Thread,
            freed_after_heap_release: true,
            address: Address::new(4),
            size: 5,
            alignment: 8,
        });
        assert_eq!(encode_payload(allocation)[6], 0x103);
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
    fn system_bytes_support_empty_storage() {
        let empty = SystemBytes::zeroed(0).unwrap();
        assert_eq!(empty.as_slice(), &[]);
        drop(empty);
    }

    #[test]
    fn fixed_buffer_reader_and_writer_reject_short_storage() {
        let mut bytes = [0_u8; 9];
        let mut writer = Writer::new(&mut bytes);
        writer.u64(1).unwrap();
        assert_eq!(writer.remaining(), &[0]);
        assert!(writer.u64(1).is_err());
        assert!(Reader::new(&[]).u64().is_err());
    }

    #[test]
    fn version_five_policies_and_legacy_thread_names_are_compatible() {
        let policies = [
            Policy {
                enabled: true,
                capture_backtraces: false,
                event_sampling: EventSampling::one_in(3).unwrap(),
            },
            Policy {
                enabled: false,
                capture_backtraces: true,
                event_sampling: EventSampling::one_in(5).unwrap(),
            },
            Policy {
                enabled: true,
                capture_backtraces: true,
                event_sampling: EventSampling::one_in(7).unwrap(),
            },
        ];
        let mut bytes = Vec::new();
        for value in policies {
            bytes.push(u8::from(value.enabled));
            bytes.push(u8::from(value.capture_backtraces));
            bytes.extend_from_slice(&0_u16.to_le_bytes());
            bytes.extend_from_slice(&u32::try_from(value.event_sampling.get()).unwrap().to_le_bytes());
        }
        let decoded = decode_recording_policies(&mut Reader::new(&bytes), 5).unwrap();
        assert_eq!(
            decoded,
            Policies {
                allocations: policies[0],
                general_events: policies[1],
                arc_dereferences: policies[2],
                runtime_tasks: policies[1],
                io: Policy::default(),
                cache: Policy::default(),
            }
        );

        let mut thread = Vec::new();
        thread.extend_from_slice(&1_u64.to_le_bytes());
        thread.extend_from_slice(&2_u64.to_le_bytes());
        thread.extend_from_slice(&3_u64.to_le_bytes());
        assert_eq!(decode_threads(&mut Reader::new(&thread), 1, 1).unwrap()[0].name, "");
    }

    #[test]
    fn version_seven_policies_default_cache_recording_to_disabled() {
        let policy = Policy {
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

        assert_eq!(decoded.cache, Policy::default());
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
    fn source_payload_can_inspect_captured_events() {
        fn capture(context: SnapshotContext<'_>) -> Result<SourceData, Error> {
            SourceData::copy_from(&context.events().total_events.to_le_bytes())
        }

        let source = Source::new(SourceId::new(106), "events", 1, capture);
        let events = Events {
            total_events: 17,
            ..Events::default()
        };
        let captured = capture_sources_from(ptr::from_ref(&source).cast_mut(), SnapshotContext { events: &events }).unwrap();

        assert_eq!(
            captured,
            vec![SourceSnapshot {
                id: SourceId::new(106),
                name: "events".into(),
                schema_version: 1,
                data: 17_u64.to_le_bytes().to_vec(),
            }]
        );
    }

    #[test]
    fn local_source_chains_detect_duplicates_and_capture_failures() {
        fn capture(_context: SnapshotContext<'_>) -> Result<SourceData, Error> {
            SourceData::copy_from(b"ok")
        }
        fn fail(_context: SnapshotContext<'_>) -> Result<SourceData, Error> {
            Err(Error::new("injected source failure"))
        }
        fn panic_during_capture(_context: SnapshotContext<'_>) -> Result<SourceData, Error> {
            panic!("injected source panic")
        }

        let first = Source::new(SourceId::new(101), "first", 1, capture);
        let second = Source::new(SourceId::new(101), "second", 1, capture);
        first.next.store(ptr::from_ref(&second).cast_mut(), Ordering::Relaxed);
        assert!(
            capture_sources_from(
                ptr::from_ref(&first).cast_mut(),
                SnapshotContext {
                    events: &Events::default(),
                }
            )
            .is_err()
        );

        let panicked = Source::new(SourceId::new(105), "panicked", 1, panic_during_capture);
        assert!(
            capture_sources_from(
                ptr::from_ref(&panicked).cast_mut(),
                SnapshotContext {
                    events: &Events::default(),
                }
            )
            .is_err()
        );

        let failed = Source::new(SourceId::new(102), "failed", 1, fail);
        assert!(
            capture_sources_from(
                ptr::from_ref(&failed).cast_mut(),
                SnapshotContext {
                    events: &Events::default(),
                }
            )
            .is_err()
        );

        let first = Source::new(SourceId::new(103), "first", 2, capture);
        let second = Source::new(SourceId::new(104), "second", 3, capture);
        first.next.store(ptr::from_ref(&second).cast_mut(), Ordering::Relaxed);
        let captured = capture_sources_from(
            ptr::from_ref(&first).cast_mut(),
            SnapshotContext {
                events: &Events::default(),
            },
        )
        .unwrap();
        assert_eq!(
            captured,
            vec![
                SourceSnapshot {
                    id: SourceId::new(103),
                    name: "first".into(),
                    schema_version: 2,
                    data: b"ok".to_vec(),
                },
                SourceSnapshot {
                    id: SourceId::new(104),
                    name: "second".into(),
                    schema_version: 3,
                    data: b"ok".to_vec(),
                },
            ]
        );
    }
    #[test]
    fn gamma_round2_snapshot_lengths_preserve_all_scripted_overflow_errors() {
        assert_eq!(snapshot_encoded_len([], [], []).unwrap(), HEADER_LEN);
        for result in [
            snapshot_encoded_len([usize::MAX], [], []),
            snapshot_encoded_len([], [(usize::MAX, 0)], []),
            snapshot_encoded_len([], [(0, usize::MAX)], []),
            snapshot_encoded_len([], [], [usize::MAX]),
            snapshot_encoded_len([], [], [usize::MAX - 26, 0]),
            snapshot_encoded_len([], [], [usize::MAX - 26]),
        ] {
            assert_eq!(result.unwrap_err().to_string(), Error::allocation_failed().to_string());
        }
        assert_eq!(snapshot_wire_count(37).unwrap(), 37);
        if let Some(overflow) = (u32::MAX as usize).checked_add(1) {
            assert_eq!(
                snapshot_wire_count(overflow).unwrap_err().to_string(),
                Error::invalid_format().to_string()
            );
        }
    }

    #[test]
    fn gamma_round2_snapshot_duration_is_exact_and_recorder_failure_is_propagated() {
        assert_eq!(snapshot_duration(|| Duration::from_nanos(37)), 37);
        assert_eq!(snapshot_duration(|| Duration::MAX), u64::MAX);
        let error = snapshot_with_recording(
            SnapshotOptions {
                event_buffers: EventBufferDisposition::Clear,
            },
            |disposition| {
                assert_eq!(disposition, EventBufferDisposition::Clear);
                Err(Error::new("scripted recorder failure"))
            },
        )
        .err()
        .unwrap();
        assert_eq!(error.to_string(), "scripted recorder failure");
    }
}
