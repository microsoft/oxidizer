// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![forbid(unsafe_code)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! On-demand runtime diagnostics for [`seismograph`].
//!
//! Register runtime and worker metadata only when diagnostics are requested by
//! the application. Each worker registers a task provider that routes bounded
//! requests to its native task storage. The provider returns owned descriptions
//! between execution cycles. There is no separate diagnostic task registry.
//!
//! Workers check [`worker::WorkerHandle::is_observed`] once per execution cycle.
//! Without collection or runtime-event recording, the normal poll loop needs no
//! diagnostic callbacks, clocks, locks, counters, or reference-count operations.
//! Allocate task identities and type descriptions only when first observed.
//!
//! Ordinary Seismograph snapshots request current task state. To measure polls,
//! call [`RuntimeHandle::capture`] with a nonzero
//! [`snapshot::CaptureOptions::measure_for`]. The diagnostic caller waits for that
//! interval, not the workers. Private observation windows prevent a poll from a
//! previous request from entering a new measurement window.
//! [`snapshot::decode`] accepts only wire version 1 and source schema 9.
//! Other versions are rejected.
//!
//! Runtime-event recording is separate from task inspection. It retains only
//! events emitted after recording starts. Missing creation times, wake ages,
//! earlier polls, and completed task history are not reconstructed.
//! Stopping event recording does not freeze the actual runtime's current state.
//! [`worker::WorkerHandle::task_events`] supplies a [`Copy`] context for task
//! creation and real outcomes, without per-task registration or shared ownership.
//!
//! # Example
//!
//! ```
//! use seismograph_runtime::RuntimeMetadata;
//! use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};
//!
//! let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("primary", 1));
//! let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
//! worker.handle().register_task_provider(|request| {
//!     // A real runtime moves this request to its worker before inspecting tasks.
//!     request.respond(
//!         Vec::new(),
//!         seismograph_runtime::snapshot::TaskCollection::Complete,
//!     );
//! })?;
//! # Ok::<(), seismograph::Error>(())
//! ```

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp, Record};
use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, TaskId, TransferId, TypeDescriptorId, WorkerId};
use seismograph::recorder::without_recording;

pub mod snapshot;
pub mod task;
pub mod worker;

mod collection;

use snapshot::{Runtime, RuntimeState, Snapshot, Worker, WorkerState};

static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_WORKER_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_TRANSFER_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_TYPE_DESCRIPTOR_ID: AtomicU64 = AtomicU64::new(1);
static REGISTRY: OnceLock<Mutex<Vec<Arc<RuntimeControl>>>> = OnceLock::new();
static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
    snapshot::source::ID,
    snapshot::source::NAME,
    snapshot::source::SCHEMA_VERSION,
    capture_source,
);

/// Metadata retained for one opted-in runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeMetadata<'a> {
    name: &'a str,
    configured_workers: u32,
    lifecycle_backtraces: BacktraceCapture,
}

impl<'a> RuntimeMetadata<'a> {
    /// Creates metadata with category-configured lifecycle backtraces.
    #[must_use]
    pub const fn new(name: &'a str, configured_workers: u32) -> Self {
        Self {
            name,
            configured_workers,
            lifecycle_backtraces: BacktraceCapture::Configured,
        }
    }

    /// Selects the lifecycle and task-spawn backtrace policy.
    #[must_use]
    pub const fn lifecycle_backtraces(mut self, capture: BacktraceCapture) -> Self {
        self.lifecycle_backtraces = capture;
        self
    }
}

/// Registration whose drop retires a runtime without reclaiming event metadata.
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

    /// Returns a shared handle for worker setup and requested collection.
    #[must_use]
    pub fn handle(&self) -> RuntimeHandle {
        self.handle.clone()
    }

    /// Registers one worker's metadata.
    #[must_use]
    pub fn register_worker(&self, metadata: worker::WorkerMetadata) -> worker::WorkerRegistration {
        self.handle.register_worker(metadata)
    }

    /// Marks the runtime as stopping.
    pub fn stopping(&self) {
        self.handle.stopping();
    }

    /// Marks the runtime as stopped.
    pub fn stopped(&self) {
        self.handle.stopped();
    }
}

impl Drop for RuntimeRegistration {
    fn drop(&mut self) {
        self.handle.stopped();
    }
}

/// Shared runtime metadata and its native-storage providers.
#[derive(Clone, Debug)]
pub struct RuntimeHandle {
    pub(crate) control: Arc<RuntimeControl>,
}

impl RuntimeHandle {
    /// Returns the process-monotonic runtime identity.
    #[must_use]
    #[inline]
    pub fn id(&self) -> RuntimeId {
        self.control.id
    }

    /// Registers worker metadata without registering any tasks.
    ///
    /// # Panics
    ///
    /// Panics if the runtime already stopped or process worker identities are exhausted.
    #[must_use]
    #[expect(
        clippy::panic,
        reason = "registering a worker after runtime retirement violates the documented caller contract"
    )]
    pub fn register_worker(&self, metadata: worker::WorkerMetadata) -> worker::WorkerRegistration {
        let worker = without_recording(|| {
            let worker = Arc::new(WorkerControl {
                id: WorkerId::from_raw(next_id(&NEXT_WORKER_ID).get()).expect("worker identities are nonzero"),
                role: metadata.role,
                processor_index: metadata.processor_index,
                state: AtomicU8::new(WorkerState::Running.wire_value()),
                thread_id: AtomicU64::new(0),
                current_task: AtomicU64::new(0),
                provider: Mutex::new(None),
                request_pending: std::sync::atomic::AtomicBool::new(false),
                measurements: Mutex::new(collection::Measurements::default()),
            });
            let mut workers = lock(&self.control.workers);
            if self.control.state.load(Ordering::Acquire) == RuntimeState::Stopped.wire_value() {
                drop(workers);
                panic!("register workers before stopping their runtime");
            }
            workers.push(Arc::clone(&worker));
            worker
        });
        record_now(
            &self.control,
            Some(worker.id),
            EventKind::WorkerStarted,
            worker.id.get(),
            0,
            u64::from(metadata.role.wire_value()),
            u64::from(metadata.processor_index.unwrap_or(u32::MAX)),
            self.control.lifecycle_backtraces,
        );
        worker::WorkerRegistration::new(self.clone(), worker)
    }

    /// Requests bounded descriptions, optionally measuring polls first.
    ///
    /// Call from a diagnostic thread, not from a worker that must answer the
    /// request. Each worker reports its own observation time and collection status.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid limits or a runtime that already stopped.
    /// Worker timeouts and provider failures appear in the returned worker rows.
    pub fn capture(&self, options: snapshot::CaptureOptions) -> Result<Runtime, seismograph::Error> {
        options.validate()?;
        if self.control.state.load(Ordering::Acquire) == RuntimeState::Stopped.wire_value() {
            return Err(seismograph::Error::new("cannot inspect a stopped runtime"));
        }
        Ok(without_recording(|| collection::capture(&self.control, options)))
    }

    /// Marks the runtime as stopping and records the transition once.
    pub fn stopping(&self) {
        stopping_with(&self.control.state, || {
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
        });
    }

    /// Retires providers and records the runtime's stop once.
    pub fn stopped(&self) {
        let at = EventTimestamp::now();
        if !self.retire_at(at) {
            return;
        }
        let workers = lock(&self.control.workers).clone();
        // Detach every callback before user-owned captures can run destructors.
        let providers = workers
            .into_iter()
            .filter_map(|worker| worker::WorkerHandle::new(self.clone(), worker).detach_provider())
            .collect::<Vec<_>>();
        record_at(
            &self.control,
            at,
            None,
            EventKind::RuntimeStopped,
            0,
            0,
            0,
            0,
            self.control.lifecycle_backtraces,
        );
        drop(providers);
    }

    fn retire_at(&self, retired_at: EventTimestamp) -> bool {
        if self
            .control
            .retired_at
            .compare_exchange(0, retired_at.ticks().max(1), Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        self.control.state.store(RuntimeState::Stopped.wire_value(), Ordering::Release);
        true
    }
}

/// Allocates an identity when native task storage is first inspected.
///
/// # Panics
///
/// Panics if process task identities are exhausted.
#[must_use]
pub fn allocate_task_id() -> TaskId {
    TaskId::from_raw(next_id(&NEXT_TASK_ID).get()).expect("task identity counters start at one")
}

/// Allocates a process-wide descriptor for a runtime's concrete task type.
///
/// # Panics
///
/// Panics if process type identities are exhausted.
#[must_use]
pub fn allocate_type_descriptor_id() -> TypeDescriptorId {
    TypeDescriptorId::from_raw(next_id(&NEXT_TYPE_DESCRIPTOR_ID).get()).expect("type identity counters start at one")
}

/// Registers metadata for an opted-in runtime without enabling recording.
///
/// # Panics
///
/// Panics if process runtime identities are exhausted.
#[must_use]
pub fn register_runtime(metadata: RuntimeMetadata<'_>) -> RuntimeRegistration {
    let control = without_recording(|| {
        seismograph::snapshot::register_source(&SOURCE);
        let control = Arc::new(RuntimeControl {
            id: RuntimeId::from_raw(next_id(&NEXT_RUNTIME_ID).get()).expect("runtime identity counters start at one"),
            name: metadata.name.to_owned(),
            configured_workers: metadata.configured_workers,
            lifecycle_backtraces: metadata.lifecycle_backtraces,
            created_at: EventTimestamp::now(),
            retired_at: AtomicU64::new(0),
            state: AtomicU8::new(RuntimeState::Running.wire_value()),
            workers: Mutex::new(Vec::new()),
            requests: AtomicUsize::new(0),
            timing_window: AtomicU64::new(0),
            timing: Mutex::new(collection::Timing::default()),
        });
        lock(registry()).push(Arc::clone(&control));
        control
    });
    record_at(
        &control,
        control.created_at,
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
pub(crate) struct RuntimeControl {
    id: RuntimeId,
    name: String,
    configured_workers: u32,
    lifecycle_backtraces: BacktraceCapture,
    created_at: EventTimestamp,
    retired_at: AtomicU64,
    state: AtomicU8,
    workers: Mutex<Vec<Arc<WorkerControl>>>,
    requests: AtomicUsize,
    timing_window: AtomicU64,
    timing: Mutex<collection::Timing>,
}

type TaskProvider = Arc<dyn Fn(task::TaskRequest) + Send + Sync>;

pub(crate) struct WorkerControl {
    id: WorkerId,
    role: worker::WorkerRole,
    processor_index: Option<u32>,
    state: AtomicU8,
    thread_id: AtomicU64,
    current_task: AtomicU64,
    provider: Mutex<Option<TaskProvider>>,
    request_pending: std::sync::atomic::AtomicBool,
    measurements: Mutex<collection::Measurements>,
}

impl std::fmt::Debug for WorkerControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("id", &self.id)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

impl WorkerControl {
    fn snapshot(&self, task_collection: snapshot::TaskCollection) -> Worker {
        Worker {
            id: self.id,
            role: self.role,
            state: WorkerState::from_wire_value(self.state.load(Ordering::Acquire)).expect("worker state stores enum values"),
            processor_index: self.processor_index,
            thread_id: NonZeroU64::new(self.thread_id.load(Ordering::Acquire))
                .map(|id| seismograph::recorder::thread::ThreadId::new(id.get())),
            current_task: TaskId::from_raw(self.current_task.load(Ordering::Acquire)),
            task_collection,
        }
    }
}

fn registry() -> &'static Mutex<Vec<Arc<RuntimeControl>>> {
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

fn capture_source(_context: seismograph::snapshot::SnapshotContext<'_>) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let runtimes = lock(registry()).clone();
    let snapshot = Snapshot {
        runtimes: runtimes
            .iter()
            .map(|runtime| collection::capture(runtime, snapshot::CaptureOptions::default()))
            .collect(),
        addresses: Vec::new(),
    };
    encode_source_with(
        &snapshot,
        snapshot::encoded_len,
        seismograph::snapshot::SourceData::zeroed,
        snapshot::encode,
    )
}

fn encode_source_with(
    snapshot: &Snapshot,
    length: impl FnOnce(&Snapshot) -> Option<usize>,
    allocate: impl FnOnce(usize) -> Result<seismograph::snapshot::SourceData, seismograph::Error>,
    encode: impl FnOnce(&Snapshot, &mut [u8]) -> Result<(), ()>,
) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let len = length(snapshot).ok_or_else(|| seismograph::Error::new("runtime source payload length overflow"))?;
    let mut data = allocate(len)?;
    encode(snapshot, data.as_mut_bytes()).map_err(|()| seismograph::Error::new("runtime source payload encoding failed"))?;
    Ok(data)
}

#[inline]
#[expect(clippy::too_many_arguments, reason = "runtime events have a fixed identity and numeric payload")]
pub(crate) fn record_now(
    control: &RuntimeControl,
    worker: Option<WorkerId>,
    kind: EventKind,
    subject: u64,
    related: u64,
    value_0: u64,
    value_1: u64,
    backtrace: BacktraceCapture,
) {
    seismograph::record(EventClass::RuntimeTask, || {
        Some(Record::runtime(
            EventTimestamp::now(),
            kind,
            RuntimeEvent {
                runtime_id: control.id,
                worker_id: worker,
                subject_id: subject,
                related_id: related,
                value_0,
                value_1,
            },
            backtrace,
        ))
    });
}

#[inline]
#[expect(
    clippy::too_many_arguments,
    reason = "runtime events have a fixed identity, timestamp, and numeric payload"
)]
pub(crate) fn record_at(
    control: &RuntimeControl,
    at: EventTimestamp,
    worker: Option<WorkerId>,
    kind: EventKind,
    subject: u64,
    related: u64,
    value_0: u64,
    value_1: u64,
    backtrace: BacktraceCapture,
) {
    seismograph::record(EventClass::RuntimeTask, || {
        Some(Record::runtime(
            at,
            kind,
            RuntimeEvent {
                runtime_id: control.id,
                worker_id: worker,
                subject_id: subject,
                related_id: related,
                value_0,
                value_1,
            },
            backtrace,
        ))
    });
}

fn next_id(counter: &AtomicU64) -> NonZeroU64 {
    let value = counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
        .expect("a process cannot exhaust u64 telemetry identities");
    NonZeroU64::new(value).expect("identity counters start at one")
}

fn stopping_with(state: &AtomicU8, record: impl FnOnce()) {
    if state
        .compare_exchange(
            RuntimeState::Running.wire_value(),
            RuntimeState::Stopping.wire_value(),
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
    {
        record();
    }
}

pub(crate) fn next_transfer_id() -> TransferId {
    TransferId::from_raw(next_id(&NEXT_TRANSFER_ID).get()).expect("transfer identity counters start at one")
}

pub(crate) fn duration_nanos(end: EventTimestamp, start: EventTimestamp) -> u64 {
    u64::try_from(end.duration_since(start).as_nanos()).unwrap_or(
        // #[gamma::skip(expr.decrement, reason = "EventTimestamp::duration_since constructs Duration::from_nanos from a saturating u64 subtraction, so as_nanos always fits u64 and this fallback is unreachable")]
        u64::MAX,
    )
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().expect("diagnostic state is never locked across user callbacks")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::WorkerRole;

    #[test]
    fn november_duration_nanos_preserves_the_full_timestamp_range_without_a_clock() {
        for (finished, started, expected) in [(0, 0, 0), (0, u64::MAX, 0), (u64::MAX, 0, u64::MAX), (u64::MAX, 1, u64::MAX - 1)] {
            assert_eq!(
                duration_nanos(EventTimestamp::from_ticks(finished), EventTimestamp::from_ticks(started)),
                expected
            );
        }
    }

    #[test]
    fn november_stopping_records_the_successful_transition_not_a_later_repeated_call() {
        let state = AtomicU8::new(RuntimeState::Running.wire_value());
        let recorded = std::cell::Cell::new(0);
        stopping_with(&state, || {
            assert_eq!(state.load(Ordering::Acquire), RuntimeState::Stopping.wire_value());
            recorded.set(recorded.get() + 1);
        });
        assert_eq!(recorded.get(), 1);
        stopping_with(&state, || panic!("a repeated stopping request must not record"));
        assert_eq!(recorded.get(), 1);
        state.store(RuntimeState::Stopped.wire_value(), Ordering::Release);
        stopping_with(&state, || panic!("a stopped runtime must not record or restart"));
        assert_eq!(state.load(Ordering::Acquire), RuntimeState::Stopped.wire_value());
    }

    #[test]
    fn gamma_round2_source_encoding_preserves_exact_length_allocation_and_encoder_errors() {
        let snapshot = Snapshot::default();
        let error = encode_source_with(
            &snapshot,
            |_| None,
            |_| panic!("overflow must not allocate"),
            |_, _| panic!("overflow must not encode"),
        )
        .err()
        .unwrap();
        assert_eq!(error.to_string(), "runtime source payload length overflow");
        let error = encode_source_with(
            &snapshot,
            |_| Some(3),
            |length| {
                assert_eq!(length, 3);
                Err(seismograph::Error::new("scripted allocation failure"))
            },
            |_, _| panic!("failed allocation must not encode"),
        )
        .err()
        .unwrap();
        assert_eq!(error.to_string(), "scripted allocation failure");
        let error = encode_source_with(
            &snapshot,
            |_| Some(3),
            seismograph::snapshot::SourceData::zeroed,
            |_, output| {
                assert_eq!(output, [0, 0, 0]);
                Err(())
            },
        )
        .err()
        .unwrap();
        assert_eq!(error.to_string(), "runtime source payload encoding failed");
        let mut data = encode_source_with(
            &snapshot,
            |_| Some(3),
            seismograph::snapshot::SourceData::zeroed,
            |value, output| {
                assert!(std::ptr::eq(value, &raw const snapshot));
                output.copy_from_slice(&[17, 23, 31]);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(data.as_mut_bytes(), [17, 23, 31]);
    }

    #[test]
    fn gamma_survivor_worker_snapshot_preserves_absent_and_present_thread_identity() {
        let worker = WorkerControl {
            id: WorkerId::from_raw(13).unwrap(),
            role: WorkerRole::Core,
            processor_index: Some(17),
            state: AtomicU8::new(1),
            thread_id: AtomicU64::new(0),
            current_task: AtomicU64::new(0),
            provider: Mutex::new(None),
            request_pending: std::sync::atomic::AtomicBool::new(false),
            measurements: Mutex::new(collection::Measurements::default()),
        };
        assert_eq!(
            format!("{worker:?}"),
            format!("{} {{ id: WorkerId(13), role: Core, .. }}", std::any::type_name::<WorkerControl>())
        );
        assert_eq!(worker.snapshot(snapshot::TaskCollection::Unavailable).thread_id, None);
        assert_eq!(worker.snapshot(snapshot::TaskCollection::Unavailable).current_task, None);
        worker.thread_id.store(19, Ordering::Relaxed);
        worker.current_task.store(23, Ordering::Relaxed);
        let captured = worker.snapshot(snapshot::TaskCollection::Complete);
        assert_eq!(captured.thread_id.unwrap().get(), 19);
        assert_eq!(captured.current_task.unwrap().get(), 23);
    }

    #[test]
    fn gamma_survivor_identity_counter_advances_once_without_wrapping() {
        let counter = AtomicU64::new(13);
        assert_eq!(next_id(&counter).get(), 13);
        assert_eq!(next_id(&counter).get(), 14);
        assert_eq!(counter.load(Ordering::Relaxed), 15);
        let boundary = AtomicU64::new(u64::MAX - 1);
        assert_eq!(next_id(&boundary).get(), u64::MAX - 1);
        std::panic::catch_unwind(|| next_id(&boundary)).unwrap_err();
        assert_eq!(boundary.load(Ordering::Relaxed), u64::MAX);
        assert_eq!(
            duration_nanos(EventTimestamp::from_ticks(u64::MAX), EventTimestamp::from_ticks(0)),
            u64::MAX
        );
        assert_eq!(duration_nanos(EventTimestamp::from_ticks(1), EventTimestamp::from_ticks(2)), 0);
    }

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn gamma_round2_retirement_uses_exact_frozen_boundaries_and_is_idempotent() {
        let _test = test_lock();
        for (at, expected) in [(0, 1), (1, 1), (37, 37)] {
            let runtime = crate::worker::gamma_round2_tests::runtime();
            assert!(runtime.retire_at(EventTimestamp::from_ticks(at)));
            assert_eq!(runtime.control.retired_at.load(Ordering::Acquire), expected);
            assert_eq!(runtime.control.state.load(Ordering::Acquire), RuntimeState::Stopped.wire_value());
            assert!(!runtime.retire_at(EventTimestamp::from_ticks(99)));
            assert_eq!(runtime.control.retired_at.load(Ordering::Acquire), expected);
        }
    }

    pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn event_durations_preserve_nanoseconds_and_saturate_backwards_time() {
        let durations = [(97, 40), (40, 97), (u64::MAX, 0)]
            .map(|(end, start)| duration_nanos(EventTimestamp::from_ticks(end), EventTimestamp::from_ticks(start)));

        assert_eq!(durations, [57, 0, u64::MAX]);
    }
}
