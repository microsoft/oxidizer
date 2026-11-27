// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime and worker source snapshot model.

use seismograph::recorder::event::{BacktraceCapture, EventTimestamp};
use seismograph::recorder::runtime::{RuntimeId, WorkerId};
use seismograph::recorder::thread::ThreadId;

use crate::WorkerRole;

const MAGIC: [u8; 8] = *b"SEISRUNT";
const WIRE_VERSION: u16 = 1;
const SCHEMA_VERSION: u16 = 10;
const HEADER_LEN: usize = 16;
const RUNTIME_FIXED_LEN: usize = 38;
const WORKER_FIXED_LEN: usize = 22;

/// Stable identity and schema metadata for the process-wide runtime source.
pub mod source {
    /// Stable source identity spelling `SEISRUNT` in ASCII.
    pub const ID: seismograph::snapshot::SourceId = seismograph::snapshot::SourceId::new(0x5345_4953_5255_4e54);
    /// Human-readable source name.
    pub(crate) const NAME: &str = "runtime";
    /// Current source payload schema version.
    pub const SCHEMA_VERSION: u16 = super::SCHEMA_VERSION;
}

/// Logical lifecycle state of a runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RuntimeState {
    /// The runtime accepts work.
    Running,
    /// The runtime is stopping.
    Stopping,
    /// The runtime has retired.
    Stopped,
}

impl RuntimeState {
    pub(crate) const fn wire_value(self) -> u8 {
        match self {
            Self::Running => 1,
            Self::Stopping => 2,
            Self::Stopped => 3,
        }
    }

    pub(crate) const fn from_wire_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Running),
            2 => Some(Self::Stopping),
            3 => Some(Self::Stopped),
            _ => None,
        }
    }
}

/// Current execution state of a runtime worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum WorkerState {
    /// The worker can execute tasks.
    Running,
    /// The worker is parked.
    Parked,
    /// The worker stopped.
    Stopped,
}

impl WorkerState {
    pub(crate) const fn wire_value(self) -> u8 {
        match self {
            Self::Running => 1,
            Self::Parked => 2,
            Self::Stopped => 3,
        }
    }

    pub(crate) const fn from_wire_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Running),
            2 => Some(Self::Parked),
            3 => Some(Self::Stopped),
            _ => None,
        }
    }
}

/// Point-in-time state of one runtime worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Worker {
    /// Process-monotonic worker identity.
    pub id: WorkerId,
    /// Worker function within the runtime.
    pub role: WorkerRole,
    /// Current worker execution state.
    pub state: WorkerState,
    /// Logical processor selected for the worker, when known.
    pub processor_index: Option<u32>,
    /// Seismograph thread recorder associated with the worker, when attached.
    pub thread_id: Option<ThreadId>,
}

/// Point-in-time state of one logical runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Runtime {
    /// Process-monotonic runtime identity.
    pub id: RuntimeId,
    /// Caller-provided diagnostic name.
    pub name: String,
    /// Number of workers requested by runtime configuration.
    pub configured_workers: u32,
    /// Backtrace policy used for lifecycle events.
    pub lifecycle_backtraces: BacktraceCapture,
    /// Current logical lifecycle state.
    pub state: RuntimeState,
    /// Runtime creation time.
    pub created_at: EventTimestamp,
    /// Runtime retirement time, when stopped.
    pub retired_at: Option<EventTimestamp>,
    /// Workers registered by this runtime.
    pub workers: Vec<Worker>,
}

/// Runtime source payload decoded from one snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    /// Active and retained retired logical runtimes.
    pub runtimes: Vec<Runtime>,
}

/// Error reported while decoding a runtime source payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Error {
    kind: ErrorKind,
}

/// Stable category of a runtime source decoding error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The source payload is truncated or internally inconsistent.
    Malformed,
    /// The private wire framing version is unsupported.
    UnsupportedWireVersion(u16),
    /// The runtime telemetry schema version is unsupported.
    UnsupportedSchemaVersion(u16),
}

impl Error {
    const fn new(kind: ErrorKind) -> Self {
        Self { kind }
    }

    /// Returns the stable error category.
    #[must_use]
    pub const fn kind(self) -> ErrorKind {
        self.kind
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            ErrorKind::Malformed => f.write_str("malformed runtime source payload"),
            ErrorKind::UnsupportedWireVersion(version) => write!(f, "unsupported runtime source wire version {version}"),
            ErrorKind::UnsupportedSchemaVersion(version) => write!(f, "unsupported runtime source schema version {version}"),
        }
    }
}

impl std::error::Error for Error {}

/// Decodes one runtime source payload.
pub fn decode(bytes: &[u8]) -> Result<Snapshot, Error> {
    let mut reader = Reader::new(bytes);
    if reader.read(MAGIC.len())? != MAGIC {
        return Err(malformed());
    }
    let wire = reader.u16()?;
    if wire != WIRE_VERSION {
        return Err(Error::new(ErrorKind::UnsupportedWireVersion(wire)));
    }
    let schema = reader.u16()?;
    if schema != SCHEMA_VERSION {
        return Err(Error::new(ErrorKind::UnsupportedSchemaVersion(schema)));
    }
    let runtime_count = reader.u32()? as usize;
    let mut runtimes = Vec::with_capacity(runtime_count);
    for _ in 0..runtime_count {
        runtimes.push(read_runtime(&mut reader)?);
    }
    if !reader.remaining().is_empty() {
        return Err(malformed());
    }
    Ok(Snapshot { runtimes })
}

pub(crate) fn encoded_len(snapshot: &Snapshot) -> Option<usize> {
    let mut len = HEADER_LEN;
    for runtime in &snapshot.runtimes {
        len = len.checked_add(RUNTIME_FIXED_LEN)?.checked_add(runtime.name.len())?;
        len = len.checked_add(runtime.workers.len().checked_mul(WORKER_FIXED_LEN)?)?;
    }
    u32::try_from(snapshot.runtimes.len()).ok()?;
    Some(len)
}

pub(crate) fn encode(snapshot: &Snapshot, output: &mut [u8]) -> Result<(), ()> {
    if encoded_len(snapshot) != Some(output.len()) {
        return Err(());
    }
    let mut writer = Writer::new(output);
    writer.write(&MAGIC)?;
    writer.u16(WIRE_VERSION)?;
    writer.u16(SCHEMA_VERSION)?;
    writer.u32(u32::try_from(snapshot.runtimes.len()).map_err(|_| ())?)?;
    for runtime in &snapshot.runtimes {
        write_runtime(&mut writer, runtime)?;
    }
    Ok(())
}

fn write_runtime(writer: &mut Writer<'_>, runtime: &Runtime) -> Result<(), ()> {
    writer.u64(runtime.id.get())?;
    writer.u32(u32::try_from(runtime.name.len()).map_err(|_| ())?)?;
    writer.u32(runtime.configured_workers)?;
    writer.u8(backtrace_wire_value(runtime.lifecycle_backtraces))?;
    writer.u8(runtime.state.wire_value())?;
    writer.u64(runtime.created_at.ticks())?;
    writer.u64(runtime.retired_at.map_or(u64::MAX, EventTimestamp::ticks))?;
    writer.u32(u32::try_from(runtime.workers.len()).map_err(|_| ())?)?;
    writer.write(runtime.name.as_bytes())?;
    for worker in &runtime.workers {
        writer.u64(worker.id.get())?;
        writer.u8(worker.role.wire_value())?;
        writer.u8(worker.state.wire_value())?;
        writer.u32(worker.processor_index.unwrap_or(u32::MAX))?;
        writer.u64(worker.thread_id.map_or(u64::MAX, ThreadId::get))?;
    }
    Ok(())
}

fn read_runtime(reader: &mut Reader<'_>) -> Result<Runtime, Error> {
    let id = RuntimeId::from_raw(reader.u64()?).ok_or_else(malformed)?;
    let name_len = reader.u32()? as usize;
    let configured_workers = reader.u32()?;
    let lifecycle_backtraces = backtrace_from_wire_value(reader.u8()?).ok_or_else(malformed)?;
    let state = RuntimeState::from_wire_value(reader.u8()?).ok_or_else(malformed)?;
    let created_at = EventTimestamp::from_ticks(reader.u64()?);
    let retired_at = optional_timestamp(reader.u64()?);
    let worker_count = reader.u32()? as usize;
    let name = std::str::from_utf8(reader.read(name_len)?).map_err(|_| malformed())?.to_owned();
    let mut workers = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        let id = WorkerId::from_raw(reader.u64()?).ok_or_else(malformed)?;
        let role = WorkerRole::from_wire_value(reader.u8()?).ok_or_else(malformed)?;
        let state = WorkerState::from_wire_value(reader.u8()?).ok_or_else(malformed)?;
        let processor_index = optional_u32(reader.u32()?);
        let thread_id = optional_thread(reader.u64()?);
        workers.push(Worker {
            id,
            role,
            state,
            processor_index,
            thread_id,
        });
    }
    Ok(Runtime {
        id,
        name,
        configured_workers,
        lifecycle_backtraces,
        state,
        created_at,
        retired_at,
        workers,
    })
}

const fn optional_u32(value: u32) -> Option<u32> {
    if value == u32::MAX { None } else { Some(value) }
}

fn optional_timestamp(value: u64) -> Option<EventTimestamp> {
    (value != u64::MAX).then(|| EventTimestamp::from_ticks(value))
}

fn optional_thread(value: u64) -> Option<ThreadId> {
    (value != u64::MAX).then(|| ThreadId::new(value))
}

const fn backtrace_wire_value(value: BacktraceCapture) -> u8 {
    match value {
        BacktraceCapture::Configured => 1,
        BacktraceCapture::Never => 2,
        BacktraceCapture::Always => 3,
    }
}

const fn backtrace_from_wire_value(value: u8) -> Option<BacktraceCapture> {
    match value {
        1 => Some(BacktraceCapture::Configured),
        2 => Some(BacktraceCapture::Never),
        3 => Some(BacktraceCapture::Always),
        _ => None,
    }
}

const fn malformed() -> Error {
    Error::new(ErrorKind::Malformed)
}

struct Writer<'a> {
    remaining: &'a mut [u8],
}

impl<'a> Writer<'a> {
    const fn new(output: &'a mut [u8]) -> Self {
        Self { remaining: output }
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if self.remaining.len() < bytes.len() {
            return Err(());
        }
        let remaining = std::mem::take(&mut self.remaining);
        let (target, rest) = remaining.split_at_mut(bytes.len());
        target.copy_from_slice(bytes);
        self.remaining = rest;
        Ok(())
    }

    fn u8(&mut self, value: u8) -> Result<(), ()> {
        self.write(&[value])
    }

    fn u16(&mut self, value: u16) -> Result<(), ()> {
        self.write(&value.to_le_bytes())
    }

    fn u32(&mut self, value: u32) -> Result<(), ()> {
        self.write(&value.to_le_bytes())
    }

    fn u64(&mut self, value: u64) -> Result<(), ()> {
        self.write(&value.to_le_bytes())
    }
}

struct Reader<'a> {
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    const fn new(input: &'a [u8]) -> Self {
        Self { remaining: input }
    }

    const fn remaining(&self) -> &'a [u8] {
        self.remaining
    }

    fn read(&mut self, len: usize) -> Result<&'a [u8], Error> {
        if self.remaining.len() < len {
            return Err(malformed());
        }
        let (value, remaining) = self.remaining.split_at(len);
        self.remaining = remaining;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.read(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.read(2)?.try_into().map_err(|_| malformed())?))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.read(4)?.try_into().map_err(|_| malformed())?))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.read(8)?.try_into().map_err(|_| malformed())?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Snapshot {
        Snapshot {
            runtimes: vec![Runtime {
                id: RuntimeId::from_raw(1).unwrap(),
                name: "primary".into(),
                configured_workers: 2,
                lifecycle_backtraces: BacktraceCapture::Configured,
                state: RuntimeState::Running,
                created_at: EventTimestamp::from_ticks(3),
                retired_at: None,
                workers: vec![Worker {
                    id: WorkerId::from_raw(4).unwrap(),
                    role: WorkerRole::Core,
                    state: WorkerState::Parked,
                    processor_index: Some(5),
                    thread_id: Some(ThreadId::new(6)),
                }],
            }],
        }
    }

    #[test]
    fn source_payload_round_trips() {
        let snapshot = fixture();
        let mut bytes = vec![0; encoded_len(&snapshot).unwrap()];
        encode(&snapshot, &mut bytes).unwrap();
        assert_eq!(decode(&bytes).unwrap(), snapshot);
    }

    #[test]
    fn malformed_and_unsupported_payloads_are_rejected() {
        let snapshot = fixture();
        let mut bytes = vec![0; encoded_len(&snapshot).unwrap()];
        encode(&snapshot, &mut bytes).unwrap();
        assert_eq!(decode(&bytes[..bytes.len() - 1]).unwrap_err().kind(), ErrorKind::Malformed);
        bytes[8..10].copy_from_slice(&2_u16.to_le_bytes());
        assert_eq!(decode(&bytes).unwrap_err().kind(), ErrorKind::UnsupportedWireVersion(2));
        bytes[8..10].copy_from_slice(&WIRE_VERSION.to_le_bytes());
        bytes[10..12].copy_from_slice(&9_u16.to_le_bytes());
        assert_eq!(decode(&bytes).unwrap_err().kind(), ErrorKind::UnsupportedSchemaVersion(9));
    }
}
