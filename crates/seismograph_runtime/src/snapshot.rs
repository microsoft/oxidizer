// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::map_err_ignore,
    reason = "Private fixed-buffer encoding intentionally collapses representability failures into one source error"
)]

//! Runtime-source snapshot model and wire encoding.

use seismograph::recorder::event::{Address, BacktraceCapture, EventTimestamp};
use seismograph::recorder::runtime::{RuntimeId, TaskId, TypeDescriptorId, WorkerId};
use seismograph::recorder::thread::ThreadId;

use crate::worker::WorkerRole;

const MAGIC: [u8; 8] = *b"SEISRUNT";
const WIRE_VERSION: u16 = 1;
const SCHEMA_VERSION: u16 = 3;
const HEADER_LEN: usize = 20;
const RUNTIME_FIXED_LEN: usize = 128;
const WORKER_FIXED_LEN: usize = 32;
const TASK_V2_FIXED_LEN: usize = 32;
const TASK_FIXED_LEN: usize = 120;
const ADDRESS_LOOKUP_FIXED_LEN: usize = 24;

/// Stable identity and schema metadata for the process-wide runtime source.
pub mod source {
    /// Stable source identity spelling `SEISRUNT` in ASCII.
    pub const ID: seismograph::snapshot::SourceId = seismograph::snapshot::SourceId::new(0x5345_4953_5255_4e54);
    /// Human-readable source name.
    pub(crate) const NAME: &str = "runtime";
    /// Current source payload schema version.
    pub(crate) const SCHEMA_VERSION: u16 = super::SCHEMA_VERSION;
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

    const fn from_wire_value(value: u8) -> Option<Self> {
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
    /// The worker is available to execute tasks.
    Running,
    /// The worker is parked.
    Parked,
    /// The worker has stopped.
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

    const fn from_wire_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Running),
            2 => Some(Self::Parked),
            3 => Some(Self::Stopped),
            _ => None,
        }
    }
}

/// Aggregate counters retained for one logical runtime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counters {
    /// Tasks assigned an identity.
    pub spawned_tasks: u64,
    /// Tasks that have not reached a terminal state.
    pub live_tasks: u64,
    /// Tasks that completed successfully.
    pub completed_tasks: u64,
    /// Tasks that were canceled.
    pub canceled_tasks: u64,
    /// Tasks that panicked.
    pub panicked_tasks: u64,
    /// Completed task polls.
    pub poll_count: u64,
    /// Nanoseconds spent polling tasks.
    pub poll_duration_nanos: u64,
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
    /// Task currently polled by the worker, when one is active.
    pub current_task: Option<TaskId>,
}

/// Spawn metadata retained for a live task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Task {
    /// Process-monotonic task identity.
    pub id: TaskId,
    /// Parent task that spawned this task, when known.
    pub parent: Option<TaskId>,
    /// Runtime-provided descriptor for the task's concrete type.
    pub type_descriptor: TypeDescriptorId,
    /// Timestamp captured when this task was registered.
    pub spawned_at: EventTimestamp,
    /// Worker that most recently polled this task, when it has run.
    pub last_worker_id: Option<WorkerId>,
    /// Lifetime counters retained independently of the bounded event ring.
    pub metrics: TaskMetrics,
    /// Call stack captured where the task was spawned.
    pub spawn_backtrace: Vec<Address>,
}

/// Lifetime execution metrics retained for one live task.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TaskMetrics {
    /// Completed task polls.
    pub poll_count: u64,
    /// Total nanoseconds spent polling the task.
    pub poll_duration_nanos: u64,
    /// Longest completed poll in nanoseconds.
    pub max_poll_duration_nanos: u64,
    /// Poll starts that followed a completed poll.
    pub resume_count: u64,
    /// Total nanoseconds from a poll finish to the following poll start.
    pub resume_duration_nanos: u64,
    /// Longest interval from a poll finish to the following poll start.
    pub max_resume_duration_nanos: u64,
    /// Poll starts that consumed a recorded wake timestamp.
    pub ready_wait_count: u64,
    /// Total nanoseconds spent runnable before a poll started.
    pub ready_wait_duration_nanos: u64,
    /// Longest interval spent runnable before a poll started.
    pub max_ready_wait_duration_nanos: u64,
}

/// Point-in-time state and retained metadata for one logical runtime.
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
    /// Timestamp captured when the runtime was registered.
    pub created_at: EventTimestamp,
    /// Timestamp captured when the runtime retired, when stopped.
    pub retired_at: Option<EventTimestamp>,
    /// Aggregate runtime counters.
    pub counters: Counters,
    /// Active and retired worker metadata.
    pub workers: Vec<Worker>,
    /// Spawn metadata for tasks that have not reached a terminal state.
    pub tasks: Vec<Task>,
}

/// Decoded payload contributed by the process-wide runtime source.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    /// Active and retained retired logical runtimes.
    pub runtimes: Vec<Runtime>,
    /// Symbols for addresses referenced only by runtime-source metadata.
    pub addresses: Vec<AddressLookup>,
}

/// Symbol information associated with a captured runtime address.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AddressLookup {
    /// Original captured address used by task spawn stacks.
    pub address: u64,
    /// Resolved symbol name, when available.
    pub symbol: Option<String>,
    /// Resolved source filename, when available.
    pub filename: Option<String>,
    /// One-based source line, when available.
    pub line: Option<u32>,
    /// One-based source column, when available.
    pub column: Option<u32>,
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
            ErrorKind::Malformed => f.write_str("the runtime telemetry source payload is malformed"),
            ErrorKind::UnsupportedWireVersion(version) => {
                write!(f, "runtime telemetry wire version {version} is unsupported")
            }
            ErrorKind::UnsupportedSchemaVersion(version) => {
                write!(f, "runtime telemetry schema version {version} is unsupported")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Decodes one process-wide runtime source payload.
///
/// # Errors
///
/// Returns an error for malformed bytes or an unsupported wire or schema
/// version. Future versions are rejected rather than silently misinterpreted.
pub fn decode(bytes: &[u8]) -> Result<Snapshot, Error> {
    let mut reader = Reader::new(bytes);
    if reader.read(MAGIC.len())? != MAGIC {
        return Err(Error::new(ErrorKind::Malformed));
    }
    let wire_version = reader.u16()?;
    if wire_version != WIRE_VERSION {
        return Err(Error::new(ErrorKind::UnsupportedWireVersion(wire_version)));
    }
    let schema_version = reader.u16()?;
    if !matches!(schema_version, 2 | SCHEMA_VERSION) {
        return Err(Error::new(ErrorKind::UnsupportedSchemaVersion(schema_version)));
    }
    let runtime_count = reader.u32()? as usize;
    let address_count = reader.u32()? as usize;
    if schema_version < 3 && address_count != 0 || runtime_count > reader.remaining().len() / RUNTIME_FIXED_LEN {
        return Err(Error::new(ErrorKind::Malformed));
    }

    let mut runtimes = Vec::with_capacity(runtime_count);
    for _ in 0..runtime_count {
        runtimes.push(read_runtime(&mut reader, schema_version)?);
    }
    if address_count > reader.remaining().len() / ADDRESS_LOOKUP_FIXED_LEN {
        return Err(Error::new(ErrorKind::Malformed));
    }
    let addresses = (0..address_count)
        .map(|_| read_address_lookup(&mut reader))
        .collect::<Result<Vec<_>, _>>()?;
    if !reader.remaining().is_empty() {
        return Err(Error::new(ErrorKind::Malformed));
    }
    Ok(Snapshot { runtimes, addresses })
}

pub(crate) fn encoded_len(snapshot: &Snapshot) -> Option<usize> {
    let runtimes_len = snapshot.runtimes.iter().try_fold(HEADER_LEN, |total, runtime| {
        let workers = runtime.workers.len().checked_mul(WORKER_FIXED_LEN)?;
        let tasks = runtime.tasks.iter().try_fold(0_usize, |total, task| {
            total
                .checked_add(TASK_FIXED_LEN)?
                .checked_add(task.spawn_backtrace.len().checked_mul(std::mem::size_of::<u64>())?)
        })?;
        total
            .checked_add(RUNTIME_FIXED_LEN)?
            .checked_add(runtime.name.len())?
            .checked_add(workers)
            .and_then(|total| total.checked_add(tasks))
    })?;
    snapshot.addresses.iter().try_fold(runtimes_len, |total, lookup| {
        total
            .checked_add(ADDRESS_LOOKUP_FIXED_LEN)?
            .checked_add(lookup.symbol.as_ref().map_or(0, String::len))?
            .checked_add(lookup.filename.as_ref().map_or(0, String::len))
    })
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
    writer.u32(u32::try_from(snapshot.addresses.len()).map_err(|_| ())?)?;
    for runtime in &snapshot.runtimes {
        write_runtime(&mut writer, runtime)?;
    }
    for lookup in &snapshot.addresses {
        write_address_lookup(&mut writer, lookup)?;
    }
    if writer.remaining().is_empty() { Ok(()) } else { Err(()) }
}

fn write_runtime(writer: &mut Writer<'_>, runtime: &Runtime) -> Result<(), ()> {
    writer.u64(runtime.id.get())?;
    writer.u8(runtime.state.wire_value())?;
    writer.u8(backtrace_wire_value(runtime.lifecycle_backtraces))?;
    writer.u16(0)?;
    writer.u32(runtime.configured_workers)?;
    writer.u64(runtime.created_at.ticks())?;
    writer.u64(runtime.retired_at.map_or(0, EventTimestamp::ticks))?;
    writer.u32(u32::try_from(runtime.name.len()).map_err(|_| ())?)?;
    writer.u32(u32::try_from(runtime.workers.len()).map_err(|_| ())?)?;
    writer.u32(u32::try_from(runtime.tasks.len()).map_err(|_| ())?)?;
    writer.u32(0)?;
    write_counters(writer, runtime.counters)?;
    writer.write(runtime.name.as_bytes())?;
    for worker in &runtime.workers {
        write_worker(writer, worker)?;
    }
    for task in &runtime.tasks {
        write_task(writer, task)?;
    }
    Ok(())
}

fn read_runtime(reader: &mut Reader<'_>, schema_version: u16) -> Result<Runtime, Error> {
    let id = RuntimeId::from_raw(reader.u64()?).ok_or_else(malformed)?;
    let state = RuntimeState::from_wire_value(reader.u8()?).ok_or_else(malformed)?;
    let lifecycle_backtraces = backtrace_from_wire_value(reader.u8()?).ok_or_else(malformed)?;
    if reader.u16()? != 0 {
        return Err(malformed());
    }
    let configured_workers = reader.u32()?;
    let created_at = EventTimestamp::from_ticks(reader.u64()?);
    let retired_at_ticks = reader.u64()?;
    let name_len = reader.u32()? as usize;
    let worker_count = reader.u32()? as usize;
    let task_count = reader.u32()? as usize;
    if reader.u32()? != 0 {
        return Err(malformed());
    }
    let counters = read_counters(reader)?;
    let name = std::str::from_utf8(reader.read(name_len)?).map_err(|_| malformed())?.to_owned();
    if worker_count > reader.remaining().len() / WORKER_FIXED_LEN {
        return Err(malformed());
    }
    let workers = (0..worker_count).map(|_| read_worker(reader)).collect::<Result<Vec<_>, _>>()?;
    let task_fixed_len = if schema_version >= 3 { TASK_FIXED_LEN } else { TASK_V2_FIXED_LEN };
    if task_count > reader.remaining().len() / task_fixed_len {
        return Err(malformed());
    }
    let tasks = (0..task_count)
        .map(|_| read_task(reader, schema_version))
        .collect::<Result<Vec<_>, _>>()?;
    let retired_at = match retired_at_ticks {
        0 => None,
        ticks => Some(EventTimestamp::from_ticks(ticks)),
    };
    if matches!(state, RuntimeState::Stopped) != retired_at.is_some() {
        return Err(malformed());
    }
    Ok(Runtime {
        id,
        name,
        configured_workers,
        lifecycle_backtraces,
        state,
        created_at,
        retired_at,
        counters,
        workers,
        tasks,
    })
}

fn write_worker(writer: &mut Writer<'_>, worker: &Worker) -> Result<(), ()> {
    writer.u64(worker.id.get())?;
    writer.u8(worker.role.wire_value())?;
    writer.u8(worker.state.wire_value())?;
    writer.u16(0)?;
    writer.u32(worker.processor_index.unwrap_or(u32::MAX))?;
    writer.u64(worker.thread_id.map_or(0, ThreadId::get))?;
    writer.u64(worker.current_task.map_or(0, TaskId::get))
}

fn read_worker(reader: &mut Reader<'_>) -> Result<Worker, Error> {
    let id = WorkerId::from_raw(reader.u64()?).ok_or_else(malformed)?;
    let role = WorkerRole::from_wire_value(reader.u8()?).ok_or_else(malformed)?;
    let state = WorkerState::from_wire_value(reader.u8()?).ok_or_else(malformed)?;
    if reader.u16()? != 0 {
        return Err(malformed());
    }
    let processor_index = match reader.u32()? {
        u32::MAX => None,
        value => Some(value),
    };
    let thread_id = match reader.u64()? {
        0 => None,
        value => Some(ThreadId::new(value)),
    };
    let current_task = match reader.u64()? {
        0 => None,
        value => Some(TaskId::from_raw(value).ok_or_else(malformed)?),
    };
    Ok(Worker {
        id,
        role,
        state,
        processor_index,
        thread_id,
        current_task,
    })
}

fn write_task(writer: &mut Writer<'_>, task: &Task) -> Result<(), ()> {
    writer.u64(task.id.get())?;
    writer.u64(task.parent.map_or(0, TaskId::get))?;
    writer.u64(task.type_descriptor.get())?;
    writer.u32(u32::try_from(task.spawn_backtrace.len()).map_err(|_| ())?)?;
    writer.u32(0)?;
    writer.u64(task.spawned_at.ticks())?;
    writer.u64(task.last_worker_id.map_or(0, WorkerId::get))?;
    writer.u64(task.metrics.poll_count)?;
    writer.u64(task.metrics.poll_duration_nanos)?;
    writer.u64(task.metrics.max_poll_duration_nanos)?;
    writer.u64(task.metrics.resume_count)?;
    writer.u64(task.metrics.resume_duration_nanos)?;
    writer.u64(task.metrics.max_resume_duration_nanos)?;
    writer.u64(task.metrics.ready_wait_count)?;
    writer.u64(task.metrics.ready_wait_duration_nanos)?;
    writer.u64(task.metrics.max_ready_wait_duration_nanos)?;
    for address in &task.spawn_backtrace {
        writer.u64(address.get())?;
    }
    Ok(())
}

fn read_task(reader: &mut Reader<'_>, schema_version: u16) -> Result<Task, Error> {
    let id = TaskId::from_raw(reader.u64()?).ok_or_else(malformed)?;
    let parent = match reader.u64()? {
        0 => None,
        value => Some(TaskId::from_raw(value).ok_or_else(malformed)?),
    };
    let type_descriptor = TypeDescriptorId::from_raw(reader.u64()?).ok_or_else(malformed)?;
    let frame_count = reader.u32()? as usize;
    if reader.u32()? != 0 || frame_count > reader.remaining().len() / std::mem::size_of::<u64>() {
        return Err(malformed());
    }
    let (spawned_at, last_worker_id, metrics) = if schema_version >= 3 {
        let spawned_at = EventTimestamp::from_ticks(reader.u64()?);
        let last_worker_id = match reader.u64()? {
            0 => None,
            value => Some(WorkerId::from_raw(value).ok_or_else(malformed)?),
        };
        (
            spawned_at,
            last_worker_id,
            TaskMetrics {
                poll_count: reader.u64()?,
                poll_duration_nanos: reader.u64()?,
                max_poll_duration_nanos: reader.u64()?,
                resume_count: reader.u64()?,
                resume_duration_nanos: reader.u64()?,
                max_resume_duration_nanos: reader.u64()?,
                ready_wait_count: reader.u64()?,
                ready_wait_duration_nanos: reader.u64()?,
                max_ready_wait_duration_nanos: reader.u64()?,
            },
        )
    } else {
        (EventTimestamp::from_ticks(0), None, TaskMetrics::default())
    };
    let spawn_backtrace = (0..frame_count)
        .map(|_| reader.u64().map(Address::new))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Task {
        id,
        parent,
        type_descriptor,
        spawned_at,
        last_worker_id,
        metrics,
        spawn_backtrace,
    })
}

fn write_address_lookup(writer: &mut Writer<'_>, lookup: &AddressLookup) -> Result<(), ()> {
    let symbol = lookup.symbol.as_deref().unwrap_or_default().as_bytes();
    let filename = lookup.filename.as_deref().unwrap_or_default().as_bytes();
    writer.u64(lookup.address)?;
    writer.u32(u32::try_from(symbol.len()).map_err(|_| ())?)?;
    writer.u32(u32::try_from(filename.len()).map_err(|_| ())?)?;
    writer.u32(lookup.line.unwrap_or(u32::MAX))?;
    writer.u32(lookup.column.unwrap_or(u32::MAX))?;
    writer.write(symbol)?;
    writer.write(filename)
}

fn read_address_lookup(reader: &mut Reader<'_>) -> Result<AddressLookup, Error> {
    let address = reader.u64()?;
    let symbol_len = reader.u32()? as usize;
    let filename_len = reader.u32()? as usize;
    let line = optional_u32(reader.u32()?);
    let column = optional_u32(reader.u32()?);
    let symbol = optional_string(reader.read(symbol_len)?)?;
    let filename = optional_string(reader.read(filename_len)?)?;
    Ok(AddressLookup {
        address,
        symbol,
        filename,
        line,
        column,
    })
}

fn optional_string(bytes: &[u8]) -> Result<Option<String>, Error> {
    if bytes.is_empty() {
        Ok(None)
    } else {
        std::str::from_utf8(bytes)
            .map(|value| Some(value.to_owned()))
            .map_err(|_| malformed())
    }
}

const fn optional_u32(value: u32) -> Option<u32> {
    if value == u32::MAX { None } else { Some(value) }
}

fn write_counters(writer: &mut Writer<'_>, counters: Counters) -> Result<(), ()> {
    writer.u64(counters.spawned_tasks)?;
    writer.u64(counters.live_tasks)?;
    writer.u64(counters.completed_tasks)?;
    writer.u64(counters.canceled_tasks)?;
    writer.u64(counters.panicked_tasks)?;
    writer.u64(counters.poll_count)?;
    writer.u64(counters.poll_duration_nanos)?;
    writer.u64(0)?;
    writer.u64(0)?;
    writer.u64(0)
}

fn read_counters(reader: &mut Reader<'_>) -> Result<Counters, Error> {
    let counters = Counters {
        spawned_tasks: reader.u64()?,
        live_tasks: reader.u64()?,
        completed_tasks: reader.u64()?,
        canceled_tasks: reader.u64()?,
        panicked_tasks: reader.u64()?,
        poll_count: reader.u64()?,
        poll_duration_nanos: reader.u64()?,
    };
    let _legacy_io_total_duration_nanos = reader.u64()?;
    let _legacy_io_wait_duration_nanos = reader.u64()?;
    let _legacy_io_active_duration_nanos = reader.u64()?;
    Ok(counters)
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
    fn new(bytes: &'a mut [u8]) -> Self {
        Self { remaining: bytes }
    }

    fn remaining(&self) -> &[u8] {
        self.remaining
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), ()> {
        let remaining = std::mem::take(&mut self.remaining);
        let Some((destination, tail)) = remaining.split_at_mut_checked(bytes.len()) else {
            return Err(());
        };
        destination.copy_from_slice(bytes);
        self.remaining = tail;
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
    const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    const fn remaining(&self) -> &'a [u8] {
        self.remaining
    }

    fn read(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let Some((value, remaining)) = self.remaining.split_at_checked(len) else {
            return Err(malformed());
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
    use super::*;

    fn fixture() -> Snapshot {
        Snapshot {
            runtimes: vec![Runtime {
                id: RuntimeId::from_raw(1).unwrap(),
                name: "primary".to_owned(),
                configured_workers: 4,
                lifecycle_backtraces: BacktraceCapture::Configured,
                state: RuntimeState::Running,
                created_at: EventTimestamp::from_ticks(10),
                retired_at: None,
                counters: Counters {
                    spawned_tasks: 7,
                    live_tasks: 2,
                    ..Counters::default()
                },
                workers: vec![Worker {
                    id: WorkerId::from_raw(2).unwrap(),
                    role: WorkerRole::Core,
                    state: WorkerState::Running,
                    processor_index: Some(3),
                    thread_id: Some(ThreadId::new(9)),
                    current_task: Some(TaskId::from_raw(4).unwrap()),
                }],
                tasks: vec![Task {
                    id: TaskId::from_raw(4).unwrap(),
                    parent: Some(TaskId::from_raw(3).unwrap()),
                    type_descriptor: TypeDescriptorId::from_raw(5).unwrap(),
                    spawned_at: EventTimestamp::from_ticks(11),
                    last_worker_id: Some(WorkerId::from_raw(2).unwrap()),
                    metrics: TaskMetrics {
                        poll_count: 12,
                        poll_duration_nanos: 13,
                        max_poll_duration_nanos: 14,
                        resume_count: 15,
                        resume_duration_nanos: 16,
                        max_resume_duration_nanos: 17,
                        ready_wait_count: 18,
                        ready_wait_duration_nanos: 19,
                        max_ready_wait_duration_nanos: 20,
                    },
                    spawn_backtrace: vec![Address::new(0x1234), Address::new(0x5678)],
                }],
            }],
            addresses: vec![AddressLookup {
                address: 0x1234,
                symbol: Some("example::spawn".into()),
                filename: Some("example.rs".into()),
                line: Some(42),
                column: Some(7),
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
    fn future_wire_version_is_rejected() {
        let mut bytes = Vec::from(MAGIC);
        bytes.extend_from_slice(&(WIRE_VERSION + 1).to_le_bytes());
        bytes.extend_from_slice(&SCHEMA_VERSION.to_le_bytes());

        assert_eq!(
            decode(&bytes).unwrap_err().kind(),
            ErrorKind::UnsupportedWireVersion(WIRE_VERSION + 1)
        );
    }

    #[test]
    fn version_two_empty_payload_remains_decodable() {
        let mut bytes = Vec::from(MAGIC);
        bytes.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());

        assert_eq!(decode(&bytes).unwrap(), Snapshot::default());
    }

    #[test]
    fn states_roles_and_backtrace_policies_round_trip() {
        let mut snapshot = fixture();
        snapshot.runtimes[0].state = RuntimeState::Stopping;
        snapshot.runtimes[0].lifecycle_backtraces = BacktraceCapture::Never;
        snapshot.runtimes[0].workers = vec![
            Worker {
                id: WorkerId::from_raw(2).unwrap(),
                role: WorkerRole::Blocking,
                state: WorkerState::Parked,
                processor_index: None,
                thread_id: None,
                current_task: None,
            },
            Worker {
                id: WorkerId::from_raw(3).unwrap(),
                role: WorkerRole::Io,
                state: WorkerState::Stopped,
                processor_index: None,
                thread_id: None,
                current_task: None,
            },
        ];
        let mut stopped = snapshot.runtimes[0].clone();
        stopped.id = RuntimeId::from_raw(4).unwrap();
        stopped.state = RuntimeState::Stopped;
        stopped.retired_at = Some(EventTimestamp::from_ticks(30));
        stopped.lifecycle_backtraces = BacktraceCapture::Always;
        stopped.workers.clear();
        stopped.tasks.clear();
        snapshot.runtimes.push(stopped);

        let mut bytes = vec![0; encoded_len(&snapshot).unwrap()];
        encode(&snapshot, &mut bytes).unwrap();
        assert_eq!(decode(&bytes).unwrap(), snapshot);
    }

    #[test]
    fn errors_have_stable_categories_and_messages() {
        let cases = [
            (
                Error::new(ErrorKind::Malformed),
                "the runtime telemetry source payload is malformed",
            ),
            (
                Error::new(ErrorKind::UnsupportedWireVersion(7)),
                "runtime telemetry wire version 7 is unsupported",
            ),
            (
                Error::new(ErrorKind::UnsupportedSchemaVersion(8)),
                "runtime telemetry schema version 8 is unsupported",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
            assert_eq!(error.kind(), error.kind);
        }
    }

    #[test]
    fn invalid_wire_discriminants_are_rejected() {
        assert_eq!(RuntimeState::from_wire_value(0), None);
        assert_eq!(WorkerState::from_wire_value(0), None);
        assert_eq!(WorkerRole::from_wire_value(0), None);
        assert_eq!(backtrace_from_wire_value(0), None);
    }

    #[test]
    fn malformed_headers_counts_and_trailing_data_are_rejected() {
        let mut invalid_magic = vec![0; HEADER_LEN];
        invalid_magic[8..10].copy_from_slice(&WIRE_VERSION.to_le_bytes());
        invalid_magic[10..12].copy_from_slice(&SCHEMA_VERSION.to_le_bytes());
        assert_eq!(decode(&invalid_magic).unwrap_err().kind(), ErrorKind::Malformed);

        let mut unsupported_schema = Vec::from(MAGIC);
        unsupported_schema.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        unsupported_schema.extend_from_slice(&(SCHEMA_VERSION + 1).to_le_bytes());
        unsupported_schema.extend_from_slice(&0_u32.to_le_bytes());
        unsupported_schema.extend_from_slice(&0_u32.to_le_bytes());
        assert_eq!(
            decode(&unsupported_schema).unwrap_err().kind(),
            ErrorKind::UnsupportedSchemaVersion(SCHEMA_VERSION + 1)
        );

        let mut legacy_addresses = Vec::from(MAGIC);
        legacy_addresses.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        legacy_addresses.extend_from_slice(&2_u16.to_le_bytes());
        legacy_addresses.extend_from_slice(&0_u32.to_le_bytes());
        legacy_addresses.extend_from_slice(&1_u32.to_le_bytes());
        assert_eq!(decode(&legacy_addresses).unwrap_err().kind(), ErrorKind::Malformed);

        let mut excessive_runtimes = Vec::from(MAGIC);
        excessive_runtimes.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        excessive_runtimes.extend_from_slice(&SCHEMA_VERSION.to_le_bytes());
        excessive_runtimes.extend_from_slice(&1_u32.to_le_bytes());
        excessive_runtimes.extend_from_slice(&0_u32.to_le_bytes());
        assert_eq!(decode(&excessive_runtimes).unwrap_err().kind(), ErrorKind::Malformed);

        let mut excessive_addresses = Vec::from(MAGIC);
        excessive_addresses.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        excessive_addresses.extend_from_slice(&SCHEMA_VERSION.to_le_bytes());
        excessive_addresses.extend_from_slice(&0_u32.to_le_bytes());
        excessive_addresses.extend_from_slice(&1_u32.to_le_bytes());
        assert_eq!(decode(&excessive_addresses).unwrap_err().kind(), ErrorKind::Malformed);

        let mut trailing = Vec::from(MAGIC);
        trailing.extend_from_slice(&WIRE_VERSION.to_le_bytes());
        trailing.extend_from_slice(&SCHEMA_VERSION.to_le_bytes());
        trailing.extend_from_slice(&0_u32.to_le_bytes());
        trailing.extend_from_slice(&0_u32.to_le_bytes());
        trailing.push(1);
        assert_eq!(decode(&trailing).unwrap_err().kind(), ErrorKind::Malformed);
    }

    #[test]
    fn malformed_runtime_worker_and_task_records_are_rejected() {
        fn encoded_fixture() -> Vec<u8> {
            let snapshot = fixture();
            let mut bytes = vec![0; encoded_len(&snapshot).unwrap()];
            encode(&snapshot, &mut bytes).unwrap();
            bytes
        }

        for (offset, value) in [(30, 1_u8), (64, 1), (165, 1), (215, 1)] {
            let mut bytes = encoded_fixture();
            bytes[offset] = value;
            assert_eq!(decode(&bytes).unwrap_err().kind(), ErrorKind::Malformed);
        }

        for (offset, value) in [(56, u32::MAX), (60, u32::MAX)] {
            let mut bytes = encoded_fixture();
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(decode(&bytes).unwrap_err().kind(), ErrorKind::Malformed);
        }

        let mut stopped_without_retirement = encoded_fixture();
        stopped_without_retirement[28] = RuntimeState::Stopped.wire_value();
        assert_eq!(decode(&stopped_without_retirement).unwrap_err().kind(), ErrorKind::Malformed);

        let mut excessive_frames = encoded_fixture();
        excessive_frames[211..215].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&excessive_frames).unwrap_err().kind(), ErrorKind::Malformed);
    }

    #[test]
    fn legacy_tasks_and_fixed_buffer_failures_are_handled() {
        let mut task = Vec::new();
        task.extend_from_slice(&1_u64.to_le_bytes());
        task.extend_from_slice(&0_u64.to_le_bytes());
        task.extend_from_slice(&2_u64.to_le_bytes());
        task.extend_from_slice(&0_u32.to_le_bytes());
        task.extend_from_slice(&0_u32.to_le_bytes());
        let decoded = read_task(&mut Reader::new(&task), 2).unwrap();
        assert_eq!(
            (decoded.spawned_at, decoded.last_worker_id, decoded.metrics),
            (EventTimestamp::from_ticks(0), None, TaskMetrics::default())
        );

        let snapshot = fixture();
        let mut short = vec![0; encoded_len(&snapshot).unwrap() - 1];
        assert_eq!(encode(&snapshot, &mut short), Err(()));

        let mut writer_bytes = [0_u8; 1];
        assert_eq!(Writer::new(&mut writer_bytes).u64(1), Err(()));
        assert_eq!(Reader::new(&[]).u64().unwrap_err().kind(), ErrorKind::Malformed);
    }
}
