// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Process-wide multi-runtime telemetry for [`seismograph`].
//!
//! One static source describes every logical runtime in the process. Runtime
//! and worker registrations own stable control blocks and retire them
//! logically, so a concurrent snapshot never dereferences reclaimed metadata.
//! Retired records are intentionally retained for process lifetime in this
//! first schema, preserving metadata for every event still present in any
//! recording session.
//!
//! # Compatibility
//!
//! The registered source has stable ID [`snapshot::source::ID`] and the name
//! `runtime`. Its private framing and public schema are independently
//! versioned. [`snapshot::decode`] rejects unknown future versions rather than
//! silently interpreting them as the current layout.
//!
//! Hot-path task, poll, transfer, and I/O methods update atomics and write the
//! calling thread's bounded Seismograph ring without formatting or allocation.
//!
//! ```
//! use seismograph_runtime::RuntimeMetadata;
//! use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};
//!
//! let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("primary", 1));
//! let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
//! worker.attach_current_thread();
//! ```

use std::collections::HashMap;
#[cfg(not(miri))]
use std::ffi::c_void;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use seismograph::recorder::SuppressionGuard;
use seismograph::recorder::event::{Address, BacktraceCapture, EventClass, EventKind, EventTimestamp, Record};
use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, TaskId, TransferId, TypeDescriptorId, WorkerId};

pub mod snapshot;
pub mod task;
pub mod worker;

use snapshot::{Counters, Runtime, RuntimeState, Snapshot, Task, Worker, WorkerState};

static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_WORKER_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_TRANSFER_ID: AtomicU64 = AtomicU64::new(1);
static REGISTRY: OnceLock<Registry> = OnceLock::new();
static ADDRESS_LOOKUPS: OnceLock<Mutex<HashMap<u64, snapshot::AddressLookup>>> = OnceLock::new();
static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
    snapshot::source::ID,
    snapshot::source::NAME,
    snapshot::source::SCHEMA_VERSION,
    capture_source,
);

/// Caller-provided metadata and configuration retained for a runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeMetadata<'a> {
    /// Diagnostic name copied under suppression during cold-path registration.
    name: &'a str,
    /// Number of workers requested by runtime configuration.
    configured_workers: u32,
    /// Backtrace policy for lifecycle, spawn, and panic events.
    lifecycle_backtraces: BacktraceCapture,
}

impl<'a> RuntimeMetadata<'a> {
    /// Creates runtime metadata with configured lifecycle backtraces.
    #[must_use]
    pub const fn new(name: &'a str, configured_workers: u32) -> Self {
        Self {
            name,
            configured_workers,
            lifecycle_backtraces: BacktraceCapture::Configured,
        }
    }

    /// Selects the backtrace policy for lifecycle, spawn, and panic events.
    #[must_use]
    pub const fn lifecycle_backtraces(mut self, capture: BacktraceCapture) -> Self {
        self.lifecycle_backtraces = capture;
        self
    }
}

/// RAII registration for one logical runtime.
///
/// Dropping the registration marks the runtime stopped and retains its metadata
/// for future snapshots.
#[derive(Debug)]
pub struct RuntimeRegistration {
    handle: RuntimeHandle,
}

impl RuntimeRegistration {
    /// Returns the process-monotonic runtime identity.
    #[must_use]
    pub fn id(&self) -> RuntimeId {
        self.handle.id()
    }

    /// Creates a cheap runtime handle for task and counter instrumentation.
    #[must_use]
    pub fn handle(&self) -> RuntimeHandle {
        self.handle.clone()
    }

    /// Creates a cheap handle for reading aggregate runtime counters.
    #[must_use]
    pub fn counters(&self) -> RuntimeCounters {
        self.handle.counters()
    }

    /// Registers a stable worker control block owned by this runtime.
    #[must_use]
    pub fn register_worker(&self, metadata: worker::WorkerMetadata) -> worker::WorkerRegistration {
        self.handle.register_worker(metadata)
    }

    /// Marks the runtime as stopping.
    pub fn stopping(&self) {
        self.handle.stopping();
    }

    /// Marks the runtime stopped before this registration is dropped.
    pub fn stopped(&self) {
        self.handle.stopped();
    }
}

impl Drop for RuntimeRegistration {
    fn drop(&mut self) {
        self.handle.stopped();
    }
}

/// Cheap shared handle for runtime events and aggregate counters.
#[derive(Clone, Debug)]
pub struct RuntimeHandle {
    pub(crate) control: Arc<RuntimeControl>,
}

impl RuntimeHandle {
    /// Returns the process-monotonic runtime identity.
    #[must_use]
    pub fn id(&self) -> RuntimeId {
        self.control.id
    }

    /// Returns a cheap handle for reading aggregate counters.
    #[must_use]
    pub fn counters(&self) -> RuntimeCounters {
        RuntimeCounters { runtime: self.clone() }
    }

    /// Registers a worker and emits [`EventKind::WorkerStarted`].
    #[must_use]
    pub fn register_worker(&self, metadata: worker::WorkerMetadata) -> worker::WorkerRegistration {
        let suppression = SuppressionGuard::enter();
        let id = next_worker_id();
        let worker = Arc::new(WorkerControl {
            id,
            role: metadata.role,
            processor_index: metadata.processor_index,
            state: AtomicU8::new(WorkerState::Running.wire_value()),
            thread_id: AtomicU64::new(0),
            current_task: AtomicU64::new(0),
        });
        lock(&self.control.workers).push(Arc::clone(&worker));
        drop(suppression);
        record_now(
            &self.control,
            Some(id),
            EventKind::WorkerStarted,
            id.get(),
            0,
            u64::from(metadata.role.wire_value()),
            u64::from(metadata.processor_index.unwrap_or(u32::MAX)),
            self.control.lifecycle_backtraces,
        );
        worker::WorkerRegistration::new(self.clone(), worker)
    }

    /// Marks the runtime as stopping and emits the transition once.
    pub fn stopping(&self) {
        if self
            .control
            .state
            .compare_exchange(
                RuntimeState::Running.wire_value(),
                RuntimeState::Stopping.wire_value(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            record_now(
                &self.control,
                None,
                EventKind::RuntimeStopping,
                0,
                0,
                0,
                0,
                self.control.lifecycle_backtraces,
            );
        }
    }

    /// Marks the runtime stopped and emits the transition once.
    pub fn stopped(&self) {
        let retired_at = EventTimestamp::now();
        if self
            .control
            .retired_at
            .compare_exchange(0, retired_at.ticks().max(1), Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.control.state.store(RuntimeState::Stopped.wire_value(), Ordering::Release);
        record_at(
            &self.control,
            retired_at,
            None,
            EventKind::RuntimeStopped,
            0,
            0,
            0,
            0,
            self.control.lifecycle_backtraces,
        );
    }

    /// Assigns a task identity, updates counters, and emits a spawn event.
    #[must_use]
    pub fn task_spawned(&self, type_descriptor: TypeDescriptorId, parent: Option<TaskId>) -> TaskId {
        self.register_task(type_descriptor, parent).id()
    }

    /// Registers a task and returns a handle for readiness telemetry.
    #[must_use]
    pub fn register_task(&self, type_descriptor: TypeDescriptorId, parent: Option<TaskId>) -> task::TaskHandle {
        let task_id = next_task_id();
        self.control.counters.spawned_tasks.fetch_add(1, Ordering::Relaxed);
        self.control.counters.live_tasks.fetch_add(1, Ordering::Relaxed);
        let spawn_backtrace = seismograph::recorder::capture_backtrace(self.control.lifecycle_backtraces);
        let suppression = SuppressionGuard::enter();
        let task = Arc::new(TaskControl {
            id: task_id,
            parent,
            type_descriptor,
            spawned_at: EventTimestamp::now(),
            spawn_backtrace,
            ready_since: AtomicU64::new(0),
            last_worker_id: AtomicU64::new(0),
            last_poll_finished_at: AtomicU64::new(0),
            poll_count: AtomicU64::new(0),
            poll_duration_nanos: AtomicU64::new(0),
            max_poll_duration_nanos: AtomicU64::new(0),
            resume_count: AtomicU64::new(0),
            resume_duration_nanos: AtomicU64::new(0),
            max_resume_duration_nanos: AtomicU64::new(0),
            ready_wait_count: AtomicU64::new(0),
            ready_wait_duration_nanos: AtomicU64::new(0),
            max_ready_wait_duration_nanos: AtomicU64::new(0),
        });
        lock(&self.control.tasks).push(Arc::clone(&task));
        drop(suppression);
        record_now(
            &self.control,
            None,
            EventKind::TaskSpawned,
            task_id.get(),
            parent.map_or(0, TaskId::get),
            type_descriptor.get(),
            0,
            self.control.lifecycle_backtraces,
        );
        task::TaskHandle::new(task)
    }

    /// Emits a task enqueue event without allocating.
    #[inline]
    pub fn task_enqueued(&self, task_id: TaskId, worker_id: Option<WorkerId>) {
        record_now(
            &self.control,
            worker_id,
            EventKind::TaskEnqueued,
            task_id.get(),
            0,
            0,
            0,
            BacktraceCapture::Never,
        );
    }

    /// Emits a task materialization event without allocating.
    #[inline]
    pub fn task_materialized(&self, task_id: TaskId, worker_id: WorkerId) {
        record_now(
            &self.control,
            Some(worker_id),
            EventKind::TaskMaterialized,
            task_id.get(),
            0,
            0,
            0,
            BacktraceCapture::Never,
        );
    }

    /// Updates terminal counters and emits a successful completion.
    #[inline]
    pub fn task_completed(&self, task_id: TaskId, worker_id: Option<WorkerId>) {
        complete_task(
            &self.control,
            worker_id,
            task_id,
            EventKind::TaskCompleted,
            &self.control.counters.completed_tasks,
            BacktraceCapture::Never,
        );
    }

    /// Updates terminal counters and emits a cancellation.
    #[inline]
    pub fn task_canceled(&self, task_id: TaskId, worker_id: Option<WorkerId>) {
        complete_task(
            &self.control,
            worker_id,
            task_id,
            EventKind::TaskCanceled,
            &self.control.counters.canceled_tasks,
            BacktraceCapture::Never,
        );
    }

    /// Updates terminal counters and emits a panic event.
    #[inline]
    pub fn task_panicked(&self, task_id: TaskId, worker_id: Option<WorkerId>) {
        complete_task(
            &self.control,
            worker_id,
            task_id,
            EventKind::TaskPanicked,
            &self.control.counters.panicked_tasks,
            self.control.lifecycle_backtraces,
        );
    }
}

/// Cheap handle for reading one runtime's aggregate counters.
#[derive(Clone, Debug)]
pub struct RuntimeCounters {
    runtime: RuntimeHandle,
}

impl RuntimeCounters {
    /// Reads a mutually consistent-enough point-in-time counter set.
    ///
    /// Individual fields are atomic; concurrent updates may appear in either
    /// order, as with the source snapshot itself.
    #[must_use]
    pub fn snapshot(&self) -> Counters {
        self.runtime.control.counters.snapshot()
    }
}

/// Registers one logical runtime in the process-wide Seismograph source.
///
/// Registration and retirement are cold paths and run under telemetry
/// suppression. The returned RAII registration owns logical retirement.
#[must_use]
pub fn register_runtime(metadata: RuntimeMetadata<'_>) -> RuntimeRegistration {
    let suppression = SuppressionGuard::enter();
    seismograph::snapshot::register_source(&SOURCE);
    let id = next_runtime_id();
    let created_at = EventTimestamp::now();
    let control = Arc::new(RuntimeControl {
        id,
        name: metadata.name.to_owned(),
        configured_workers: metadata.configured_workers,
        lifecycle_backtraces: metadata.lifecycle_backtraces,
        created_at,
        retired_at: AtomicU64::new(0),
        state: AtomicU8::new(RuntimeState::Running.wire_value()),
        counters: CounterBlock::default(),
        workers: Mutex::new(Vec::new()),
        tasks: Mutex::new(Vec::new()),
    });
    lock(&registry().runtimes).push(Arc::clone(&control));
    drop(suppression);
    record_at(
        &control,
        created_at,
        None,
        EventKind::RuntimeCreated,
        0,
        0,
        u64::from(control.configured_workers),
        0,
        control.lifecycle_backtraces,
    );
    RuntimeRegistration {
        handle: RuntimeHandle { control },
    }
}

#[derive(Debug)]
struct Registry {
    runtimes: Mutex<Vec<Arc<RuntimeControl>>>,
}

#[derive(Debug)]
pub(crate) struct RuntimeControl {
    id: RuntimeId,
    name: String,
    configured_workers: u32,
    pub(crate) lifecycle_backtraces: BacktraceCapture,
    created_at: EventTimestamp,
    retired_at: AtomicU64,
    state: AtomicU8,
    pub(crate) counters: CounterBlock,
    workers: Mutex<Vec<Arc<WorkerControl>>>,
    tasks: Mutex<Vec<Arc<TaskControl>>>,
}

#[derive(Debug)]
pub(crate) struct WorkerControl {
    pub(crate) id: WorkerId,
    role: worker::WorkerRole,
    processor_index: Option<u32>,
    pub(crate) state: AtomicU8,
    pub(crate) thread_id: AtomicU64,
    pub(crate) current_task: AtomicU64,
}

#[derive(Debug)]
pub(crate) struct TaskControl {
    pub(crate) id: TaskId,
    parent: Option<TaskId>,
    type_descriptor: TypeDescriptorId,
    spawned_at: EventTimestamp,
    spawn_backtrace: Vec<Address>,
    pub(crate) ready_since: AtomicU64,
    pub(crate) last_worker_id: AtomicU64,
    pub(crate) last_poll_finished_at: AtomicU64,
    pub(crate) poll_count: AtomicU64,
    pub(crate) poll_duration_nanos: AtomicU64,
    pub(crate) max_poll_duration_nanos: AtomicU64,
    pub(crate) resume_count: AtomicU64,
    pub(crate) resume_duration_nanos: AtomicU64,
    pub(crate) max_resume_duration_nanos: AtomicU64,
    pub(crate) ready_wait_count: AtomicU64,
    pub(crate) ready_wait_duration_nanos: AtomicU64,
    pub(crate) max_ready_wait_duration_nanos: AtomicU64,
}

#[derive(Debug, Default)]
pub(crate) struct CounterBlock {
    spawned_tasks: AtomicU64,
    live_tasks: AtomicU64,
    completed_tasks: AtomicU64,
    canceled_tasks: AtomicU64,
    panicked_tasks: AtomicU64,
    pub(crate) poll_count: AtomicU64,
    pub(crate) poll_duration_nanos: AtomicU64,
}

impl CounterBlock {
    fn snapshot(&self) -> Counters {
        Counters {
            spawned_tasks: self.spawned_tasks.load(Ordering::Relaxed),
            live_tasks: self.live_tasks.load(Ordering::Relaxed),
            completed_tasks: self.completed_tasks.load(Ordering::Relaxed),
            canceled_tasks: self.canceled_tasks.load(Ordering::Relaxed),
            panicked_tasks: self.panicked_tasks.load(Ordering::Relaxed),
            poll_count: self.poll_count.load(Ordering::Relaxed),
            poll_duration_nanos: self.poll_duration_nanos.load(Ordering::Relaxed),
        }
    }
}

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| Registry {
        runtimes: Mutex::new(Vec::new()),
    })
}

fn capture_source(_context: seismograph::snapshot::SnapshotContext<'_>) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let snapshot = capture_registry();
    let len = snapshot::encoded_len(&snapshot).ok_or_else(|| seismograph::Error::new("runtime source payload length overflow"))?;
    let mut data = seismograph::snapshot::SourceData::zeroed(len)?;
    snapshot::encode(&snapshot, data.as_mut_bytes()).map_err(|()| seismograph::Error::new("runtime source payload encoding failed"))?;
    Ok(data)
}

fn capture_registry() -> Snapshot {
    let runtimes = lock(&registry().runtimes);
    let runtimes = runtimes.iter().map(|runtime| runtime.snapshot()).collect::<Vec<_>>();
    let addresses = resolve_runtime_addresses(&runtimes);
    Snapshot { runtimes, addresses }
}

fn resolve_runtime_addresses(runtimes: &[Runtime]) -> Vec<snapshot::AddressLookup> {
    let mut addresses = runtimes
        .iter()
        .flat_map(|runtime| &runtime.tasks)
        .flat_map(|task| &task.spawn_backtrace)
        .map(|address| address.get())
        .filter(|address| *address != 0)
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    let _suppression = SuppressionGuard::enter();
    seismograph::snapshot::with_snapshot_arena_suspended(|| {
        let mut cache = lock(ADDRESS_LOOKUPS.get_or_init(|| Mutex::new(HashMap::new())));
        for &address in &addresses {
            if cache.contains_key(&address) {
                continue;
            }
            let mut lookup = snapshot::AddressLookup {
                address,
                ..snapshot::AddressLookup::default()
            };
            #[cfg(not(miri))]
            backtrace::resolve(
                seismograph::recorder::symbol_lookup_address(Address::new(address)).get() as *mut c_void,
                |symbol| {
                    lookup.symbol = lookup.symbol.take().or_else(|| symbol.name().map(|name| name.to_string()));
                    lookup.filename = lookup
                        .filename
                        .take()
                        .or_else(|| symbol.filename().map(|path| path.to_string_lossy().into_owned()));
                    lookup.line = lookup.line.or_else(|| symbol.lineno());
                    lookup.column = lookup.column.or_else(|| symbol.colno());
                },
            );
            cache.insert(address, lookup);
        }
    });
    let cache = lock(ADDRESS_LOOKUPS.get_or_init(|| Mutex::new(HashMap::new())));
    addresses.into_iter().map(|address| cache[&address].clone()).collect()
}

impl RuntimeControl {
    fn snapshot(&self) -> Runtime {
        let state = runtime_state(self.state.load(Ordering::Acquire));
        let retired_at = match state {
            RuntimeState::Stopped => Some(EventTimestamp::from_ticks(self.retired_at.load(Ordering::Acquire))),
            RuntimeState::Running | RuntimeState::Stopping => None,
        };
        let workers = lock(&self.workers).iter().map(|worker| worker.snapshot()).collect();
        let tasks = lock(&self.tasks).iter().map(|task| task.snapshot()).collect();
        Runtime {
            id: self.id,
            name: self.name.clone(),
            configured_workers: self.configured_workers,
            lifecycle_backtraces: self.lifecycle_backtraces,
            state,
            created_at: self.created_at,
            retired_at,
            counters: self.counters.snapshot(),
            workers,
            tasks,
        }
    }
}

impl TaskControl {
    fn snapshot(&self) -> Task {
        Task {
            id: self.id,
            parent: self.parent,
            type_descriptor: self.type_descriptor,
            spawned_at: self.spawned_at,
            last_worker_id: WorkerId::from_raw(self.last_worker_id.load(Ordering::Acquire)),
            metrics: snapshot::TaskMetrics {
                poll_count: self.poll_count.load(Ordering::Relaxed),
                poll_duration_nanos: self.poll_duration_nanos.load(Ordering::Relaxed),
                max_poll_duration_nanos: self.max_poll_duration_nanos.load(Ordering::Relaxed),
                resume_count: self.resume_count.load(Ordering::Relaxed),
                resume_duration_nanos: self.resume_duration_nanos.load(Ordering::Relaxed),
                max_resume_duration_nanos: self.max_resume_duration_nanos.load(Ordering::Relaxed),
                ready_wait_count: self.ready_wait_count.load(Ordering::Relaxed),
                ready_wait_duration_nanos: self.ready_wait_duration_nanos.load(Ordering::Relaxed),
                max_ready_wait_duration_nanos: self.max_ready_wait_duration_nanos.load(Ordering::Relaxed),
            },
            spawn_backtrace: self.spawn_backtrace.clone(),
        }
    }
}

impl WorkerControl {
    fn snapshot(&self) -> Worker {
        Worker {
            id: self.id,
            role: self.role,
            state: worker_state(self.state.load(Ordering::Acquire)),
            processor_index: self.processor_index,
            thread_id: match self.thread_id.load(Ordering::Acquire) {
                0 => None,
                value => Some(seismograph::recorder::thread::ThreadId::new(value)),
            },
            current_task: match self.current_task.load(Ordering::Acquire) {
                0 => None,
                value => TaskId::from_raw(value),
            },
        }
    }
}

fn complete_task(
    control: &RuntimeControl,
    worker_id: Option<WorkerId>,
    task_id: TaskId,
    kind: EventKind,
    terminal_counter: &AtomicU64,
    backtrace: BacktraceCapture,
) {
    decrement_saturating(&control.counters.live_tasks);
    terminal_counter.fetch_add(1, Ordering::Relaxed);
    let suppression = SuppressionGuard::enter();
    lock(&control.tasks).retain(|task| task.id != task_id);
    drop(suppression);
    record_now(control, worker_id, kind, task_id.get(), 0, 0, 0, backtrace);
}

fn decrement_saturating(value: &AtomicU64) {
    let _previous = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_sub(1));
}

#[inline]
#[expect(
    clippy::too_many_arguments,
    reason = "The fixed runtime event payload has two identities and two numeric values"
)]
pub(crate) fn record_now(
    control: &RuntimeControl,
    worker_id: Option<WorkerId>,
    kind: EventKind,
    subject_id: u64,
    related_id: u64,
    value_0: u64,
    value_1: u64,
    backtrace: BacktraceCapture,
) {
    seismograph::record(EventClass::RuntimeTask, || {
        runtime_record(
            EventTimestamp::now(),
            control.id,
            worker_id,
            kind,
            subject_id,
            related_id,
            value_0,
            value_1,
            backtrace,
        )
    });
}

#[inline]
#[expect(
    clippy::too_many_arguments,
    reason = "The fixed runtime event payload has two identities and two numeric values"
)]
pub(crate) fn record_at(
    control: &RuntimeControl,
    timestamp: EventTimestamp,
    worker_id: Option<WorkerId>,
    kind: EventKind,
    subject_id: u64,
    related_id: u64,
    value_0: u64,
    value_1: u64,
    backtrace: BacktraceCapture,
) {
    seismograph::record(EventClass::RuntimeTask, || {
        runtime_record(
            timestamp, control.id, worker_id, kind, subject_id, related_id, value_0, value_1, backtrace,
        )
    });
}

#[expect(
    clippy::too_many_arguments,
    reason = "The fixed runtime event payload has two identities and two numeric values"
)]
const fn runtime_record(
    timestamp: EventTimestamp,
    runtime_id: RuntimeId,
    worker_id: Option<WorkerId>,
    kind: EventKind,
    subject_id: u64,
    related_id: u64,
    value_0: u64,
    value_1: u64,
    backtrace: BacktraceCapture,
) -> Record {
    Record::runtime(
        timestamp,
        kind,
        RuntimeEvent {
            runtime_id,
            worker_id,
            subject_id,
            related_id,
            value_0,
            value_1,
        },
        backtrace,
    )
}

pub(crate) fn duration_nanos(finished_at: EventTimestamp, started_at: EventTimestamp) -> u64 {
    u64::try_from(finished_at.duration_since(started_at).as_nanos()).unwrap_or(u64::MAX)
}

fn runtime_state(value: u8) -> RuntimeState {
    match value {
        1 => RuntimeState::Running,
        2 => RuntimeState::Stopping,
        3 => RuntimeState::Stopped,
        _ => unreachable!("runtime telemetry state atomics are only written from RuntimeState values"),
    }
}

fn worker_state(value: u8) -> WorkerState {
    match value {
        1 => WorkerState::Running,
        2 => WorkerState::Parked,
        3 => WorkerState::Stopped,
        _ => unreachable!("worker telemetry state atomics are only written from WorkerState values"),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .expect("runtime telemetry metadata mutex poisoning means a cold-path mutation panicked while holding the lock")
}

fn next_id(counter: &AtomicU64) -> NonZeroU64 {
    let value = counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(1))
        .expect("a process cannot create u64::MAX runtime telemetry identities");
    NonZeroU64::new(value).expect("runtime telemetry identifier counters are initialized to one")
}

fn next_runtime_id() -> RuntimeId {
    RuntimeId::from_raw(next_id(&NEXT_RUNTIME_ID).get()).expect("next_id always returns a nonzero runtime identifier")
}

fn next_worker_id() -> WorkerId {
    WorkerId::from_raw(next_id(&NEXT_WORKER_ID).get()).expect("next_id always returns a nonzero worker identifier")
}

fn next_task_id() -> TaskId {
    TaskId::from_raw(next_id(&NEXT_TASK_ID).get()).expect("next_id always returns a nonzero task identifier")
}

pub(crate) fn next_transfer_id() -> TransferId {
    TransferId::from_raw(next_id(&NEXT_TRANSFER_ID).get()).expect("next_id always returns a nonzero transfer identifier")
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::worker::{WorkerMetadata, WorkerRole};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn type_descriptor_id(value: u64) -> TypeDescriptorId {
        TypeDescriptorId::from_raw(value).unwrap()
    }

    fn source_snapshot() -> Snapshot {
        let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        let source = decoded.sources.iter().find(|source| source.id == snapshot::source::ID).unwrap();
        snapshot::decode(&source.data).unwrap()
    }

    #[test]
    #[expect(
        clippy::needless_collect,
        reason = "all registration threads must start before joins serialize their completion"
    )]
    fn concurrent_runtime_registrations_have_unique_ids() {
        let _test = TEST_LOCK.lock().unwrap();
        let registrations = (0..16)
            .map(|index| {
                thread::spawn(move || {
                    let name = format!("concurrent-{index}");
                    let runtime = register_runtime(RuntimeMetadata::new(&name, 1));
                    runtime.id()
                })
            })
            .collect::<Vec<_>>();
        let ids = registrations
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(ids.len(), 16);
    }

    #[test]
    fn worker_association_and_retirement_are_visible() {
        let _test = TEST_LOCK.lock().unwrap();
        let runtime = register_runtime(RuntimeMetadata::new("worker-test", 1));
        let runtime_id = runtime.id();
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core).processor_index(3));
        let worker_id = worker.id();
        worker.attach_current_thread();
        drop(worker);
        drop(runtime);

        let snapshot = source_snapshot();
        let runtime = snapshot.runtimes.iter().find(|runtime| runtime.id == runtime_id).unwrap();
        let worker = runtime.workers.iter().find(|worker| worker.id == worker_id).unwrap();
        assert_eq!(
            (runtime.state, runtime.retired_at.is_some(), worker.state, worker.processor_index),
            (RuntimeState::Stopped, true, WorkerState::Stopped, Some(3))
        );
    }

    #[test]
    fn task_poll_counters_accumulate() {
        let _test = TEST_LOCK.lock().unwrap();
        let runtime = register_runtime(RuntimeMetadata::new("counters", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().task_spawned(type_descriptor_id(1), None);
        let poll = worker.handle().task_poll_started(task);
        worker.handle().task_poll_finished(poll);
        runtime.handle().task_completed(task, Some(worker.id()));

        let counters = runtime.handle().counters().snapshot();
        assert_eq!(
            (
                counters.spawned_tasks,
                counters.live_tasks,
                counters.completed_tasks,
                counters.poll_count,
            ),
            (1, 0, 1, 1)
        );
    }

    #[test]
    fn task_readiness_retains_first_wake_until_poll() {
        let _test = TEST_LOCK.lock().unwrap();
        seismograph::recorder(seismograph::recorder::Configuration {
            runtime_tasks: seismograph::recorder::RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let runtime = register_runtime(RuntimeMetadata::new("readiness", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_task(type_descriptor_id(1), None);

        task.woken();
        let first_wake = task.task.ready_since.load(Ordering::Relaxed);
        task.woken();
        assert_eq!(task.task.ready_since.load(Ordering::Relaxed), first_wake);

        let poll = task.poll_started(&worker.handle());
        task.poll_finished(&worker.handle(), poll);
        let poll = task.poll_started(&worker.handle());
        task.poll_finished(&worker.handle(), poll);
        let runtime_snapshot = source_snapshot();
        let task_snapshot = runtime_snapshot
            .runtimes
            .iter()
            .find(|candidate| candidate.id == runtime.id())
            .and_then(|runtime| runtime.tasks.iter().find(|candidate| candidate.id == task.id()))
            .unwrap();
        assert_eq!(
            (
                task_snapshot.metrics.poll_count,
                task_snapshot.metrics.resume_count,
                task_snapshot.metrics.ready_wait_count,
            ),
            (2, 1, 1)
        );
        assert!(task_snapshot.metrics.poll_duration_nanos > 0);
        assert!(task_snapshot.metrics.max_poll_duration_nanos > 0);
        assert!(task_snapshot.metrics.resume_duration_nanos > 0);
        assert!(task_snapshot.metrics.max_resume_duration_nanos > 0);
        assert!(task_snapshot.metrics.ready_wait_duration_nanos > 0);
        assert!(task_snapshot.metrics.max_ready_wait_duration_nanos > 0);
        runtime.handle().task_completed(task.id(), Some(worker.id()));

        let encoded = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let poll_starts = seismograph::snapshot::decode(encoded.as_bytes())
            .unwrap()
            .events
            .events
            .into_iter()
            .filter(|event| event.kind == EventKind::TaskPollStarted)
            .filter_map(|event| event.runtime())
            .filter(|event| event.subject_id == task.id().get())
            .collect::<Vec<_>>();

        assert_eq!(poll_starts.len(), 2);
        assert_eq!(poll_starts[0].value_1, 1);
        assert!(poll_starts[0].value_0 > 0);
        assert_eq!((poll_starts[1].value_0, poll_starts[1].value_1), (0, 0));
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }

    #[cfg(not(miri))]
    #[test]
    fn live_task_snapshot_retains_spawn_backtrace() {
        let _test = TEST_LOCK.lock().unwrap();
        seismograph::recorder(seismograph::recorder::Configuration {
            runtime_tasks: seismograph::recorder::RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let runtime = register_runtime(RuntimeMetadata::new("spawn-backtrace", 1));
        let type_descriptor = type_descriptor_id(1);
        let task_id = runtime.handle().task_spawned(type_descriptor, None);

        let first_snapshot = source_snapshot();
        let task = first_snapshot
            .runtimes
            .iter()
            .find(|candidate| candidate.id == runtime.id())
            .and_then(|runtime| runtime.tasks.iter().find(|task| task.id == task_id))
            .unwrap();

        assert_eq!((task.type_descriptor, task.spawn_backtrace.is_empty()), (type_descriptor, false));
        assert!(
            task.spawn_backtrace
                .iter()
                .all(|frame| first_snapshot.addresses.iter().any(|lookup| lookup.address == frame.get()))
        );
        assert!(
            first_snapshot
                .addresses
                .iter()
                .any(|lookup| lookup.symbol.is_some() || lookup.filename.is_some())
        );
        let second_snapshot = source_snapshot();
        assert_eq!(second_snapshot.addresses, first_snapshot.addresses);
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }

    #[test]
    fn typed_api_records_fixed_runtime_payloads() {
        let _test = TEST_LOCK.lock().unwrap();
        seismograph::recorder(seismograph::recorder::Configuration {
            runtime_tasks: seismograph::recorder::RecordingPolicy {
                enabled: true,
                capture_backtraces: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let runtime = register_runtime(RuntimeMetadata::new("events", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().task_spawned(type_descriptor_id(1), None);
        runtime.handle().task_enqueued(task, Some(worker.id()));
        let poll = worker.handle().task_poll_started(task);
        worker.handle().task_poll_finished(poll);
        let encoded = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let events = seismograph::snapshot::decode(encoded.as_bytes()).unwrap().events.events;
        let enqueued = events
            .iter()
            .find(|event| event.kind == EventKind::TaskEnqueued && event.runtime().is_some_and(|context| context.subject_id == task.get()))
            .unwrap();
        let spawned = events
            .iter()
            .find(|event| event.kind == EventKind::TaskSpawned && event.runtime().is_some_and(|context| context.subject_id == task.get()))
            .unwrap();

        assert_eq!(
            (
                enqueued.runtime().unwrap().worker_id,
                enqueued.call_stack.is_empty(),
                spawned.call_stack.is_empty(),
            ),
            (Some(worker.id()), true, false)
        );
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }

    #[test]
    fn concurrent_snapshot_and_drop_are_safe() {
        let _test = TEST_LOCK.lock().unwrap();
        let registrations = (0..16)
            .map(|index| {
                let name = format!("runtime-{index}");
                register_runtime(RuntimeMetadata::new(&name, 1))
            })
            .collect::<Vec<_>>();
        let snapshotter = thread::spawn(|| {
            for _ in 0..16 {
                let _snapshot = source_snapshot();
            }
        });
        drop(registrations);

        snapshotter.join().unwrap();
    }

    #[test]
    fn lifecycle_transfer_and_terminal_paths_are_visible() {
        let _test = TEST_LOCK.lock().unwrap();
        seismograph::recorder(seismograph::recorder::Configuration {
            runtime_tasks: seismograph::recorder::RecordingPolicy {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let runtime = register_runtime(RuntimeMetadata::new("lifecycle", 2).lifecycle_backtraces(BacktraceCapture::Always));
        let runtime_id = runtime.id();
        let source = runtime.register_worker(WorkerMetadata::new(WorkerRole::Blocking));
        let destination = runtime.register_worker(WorkerMetadata::new(WorkerRole::Io));
        source.handle().parked();
        let parked = source_snapshot();
        let runtime_snapshot = parked.runtimes.iter().find(|candidate| candidate.id == runtime_id).unwrap();
        assert_eq!(
            runtime_snapshot
                .workers
                .iter()
                .find(|worker| worker.id == source.id())
                .unwrap()
                .state,
            WorkerState::Parked
        );
        source.handle().unparked();

        let handle = runtime.handle();
        let transferred = handle.register_task(type_descriptor_id(1), None);
        let transfer = source.handle().transfer_started(transferred.id(), destination.id());
        source.handle().instance_relocated(&transfer);
        source.handle().transfer_finished(transfer);
        handle.task_materialized(transferred.id(), destination.id());
        let poll = transferred.poll_started(&destination.handle());
        let active = source_snapshot();
        assert_eq!(
            active
                .runtimes
                .iter()
                .find(|candidate| candidate.id == runtime_id)
                .unwrap()
                .workers
                .iter()
                .find(|worker| worker.id == destination.id())
                .unwrap()
                .current_task,
            Some(transferred.id())
        );
        transferred.poll_finished(&destination.handle(), poll);
        handle.task_canceled(transferred.id(), Some(destination.id()));

        let panicked = handle.register_task(type_descriptor_id(2), None);
        handle.task_panicked(panicked.id(), Some(source.id()));
        handle.task_panicked(panicked.id(), Some(source.id()));
        assert_eq!(
            (
                runtime.counters().snapshot().canceled_tasks,
                runtime.counters().snapshot().panicked_tasks,
            ),
            (1, 2)
        );

        runtime.stopping();
        runtime.stopping();
        let stopping = source_snapshot();
        assert_eq!(
            stopping.runtimes.iter().find(|candidate| candidate.id == runtime_id).unwrap().state,
            RuntimeState::Stopping
        );
        runtime.stopped();
        runtime.stopped();
        drop(runtime);

        let retired = source_snapshot();
        assert_eq!(
            retired.runtimes.iter().find(|candidate| candidate.id == runtime_id).unwrap().state,
            RuntimeState::Stopped
        );
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }

    #[test]
    fn invalid_internal_states_panic() {
        std::panic::catch_unwind(|| runtime_state(0)).unwrap_err();
        std::panic::catch_unwind(|| worker_state(0)).unwrap_err();
    }
}
