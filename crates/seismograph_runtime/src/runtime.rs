// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime registration, lifecycle events, and snapshot collection.

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::panic::Location;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp, Record};
use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, TaskId, TypeDescriptorId, WorkerId};
use seismograph::recorder::without_recording;

use crate::snapshot::{Runtime as RuntimeSnapshot, RuntimeState, Snapshot, TaskLocation, Worker, WorkerState};
use crate::{TaskIdentity, WorkerMetadata, WorkerRole, asynchronous, blocking, snapshot, worker};

static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_WORKER_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);
static REGISTRY: Mutex<Vec<Arc<RuntimeControl>>> = Mutex::new(Vec::new());
static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
    snapshot::source::ID,
    snapshot::source::NAME,
    snapshot::source::SCHEMA_VERSION,
    capture_source,
);

/// Builds one runtime diagnostics registration.
#[derive(Clone, Debug)]
pub struct RuntimeBuilder {
    name: String,
    configured_workers: u32,
    lifecycle_backtraces: BacktraceCapture,
}

impl RuntimeBuilder {
    /// Creates a runtime builder with no configured workers.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            configured_workers: 0,
            lifecycle_backtraces: BacktraceCapture::Configured,
        }
    }

    /// Sets the number of workers requested by runtime configuration.
    #[must_use]
    pub const fn configured_workers(mut self, configured_workers: u32) -> Self {
        self.configured_workers = configured_workers;
        self
    }

    /// Selects the lifecycle and task backtrace policy.
    #[must_use]
    pub const fn lifecycle_backtraces(mut self, capture: BacktraceCapture) -> Self {
        self.lifecycle_backtraces = capture;
        self
    }

    /// Registers and returns the runtime.
    ///
    /// # Panics
    ///
    /// Panics if process runtime identities are exhausted.
    #[must_use]
    pub fn build(self) -> Runtime {
        Runtime {
            owner: Arc::new(RuntimeOwner {
                handle: build_runtime(self),
            }),
        }
    }
}

/// Runtime diagnostics registration and task lifecycle entry point.
#[derive(Clone, Debug)]
pub struct Runtime {
    owner: Arc<RuntimeOwner>,
}

#[derive(Debug)]
pub(crate) struct RuntimeOwner {
    pub(crate) handle: RuntimeHandle,
}

impl Runtime {
    /// Creates a runtime builder.
    #[must_use]
    pub fn builder(name: impl Into<String>) -> RuntimeBuilder {
        RuntimeBuilder::new(name)
    }

    /// Returns the process-monotonic runtime identity.
    #[must_use]
    pub fn id(&self) -> RuntimeId {
        self.owner.handle.id()
    }

    /// Creates an asynchronous worker on the current thread.
    ///
    /// # Panics
    ///
    /// Panics if metadata uses [`WorkerRole::Blocking`] or the runtime stopped.
    #[must_use]
    pub fn async_worker(&self, metadata: WorkerMetadata) -> asynchronous::Worker {
        assert_ne!(
            metadata.role,
            WorkerRole::Blocking,
            "an asynchronous worker cannot use the blocking role"
        );
        asynchronous::Worker::new(self.clone(), self.owner.handle.register_worker(metadata))
    }

    /// Creates a blocking worker on the current thread.
    ///
    /// # Panics
    ///
    /// Panics if metadata does not use [`WorkerRole::Blocking`] or the runtime stopped.
    #[must_use]
    pub fn blocking_worker(&self, metadata: WorkerMetadata) -> blocking::Worker {
        assert_eq!(metadata.role, WorkerRole::Blocking, "a blocking worker requires the blocking role");
        blocking::Worker::new(self.clone(), self.owner.handle.register_worker(metadata))
    }

    /// Creates an asynchronous task handle before queue insertion.
    #[must_use]
    pub fn async_task(&self, identity: TaskIdentity, body_size: usize) -> Option<asynchronous::TaskHandle> {
        let task = asynchronous::TaskHandle::new(self.id(), self.owner.handle.control.lifecycle_backtraces, identity, body_size);
        if task.is_some() {
            self.owner.handle.register_task_identity(identity);
        }
        task
    }

    /// Creates a blocking task handle before queue insertion.
    #[must_use]
    pub fn blocking_task(&self, identity: TaskIdentity, body_size: usize) -> Option<blocking::TaskHandle> {
        let task = blocking::TaskHandle::new(self.id(), self.owner.handle.control.lifecycle_backtraces, identity, body_size);
        if task.is_some() {
            self.owner.handle.register_task_identity(identity);
        }
        task
    }

    /// Captures the runtime, its registered workers, and task spawn locations.
    #[must_use]
    pub fn capture(&self) -> RuntimeSnapshot {
        self.owner.handle.capture()
    }

    /// Marks the runtime as stopping.
    pub fn stopping(&self) {
        self.owner.handle.stopping();
    }

    /// Stops the runtime and retires its workers.
    pub fn stop(&self) {
        self.owner.handle.stopped();
    }
}

impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        self.handle.stopped();
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeHandle {
    pub(crate) control: Arc<RuntimeControl>,
}

impl RuntimeHandle {
    pub(crate) fn id(&self) -> RuntimeId {
        self.control.id
    }

    fn register_task_identity(&self, identity: TaskIdentity) {
        without_recording(|| {
            lock(&self.control.task_locations)
                .entry(identity.descriptor())
                .or_insert(identity.location());
        });
    }

    #[expect(
        clippy::panic,
        reason = "registering a worker after runtime retirement violates the documented caller contract"
    )]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "worker registration consumes the metadata supplied for that new worker"
    )]
    pub(crate) fn register_worker(&self, metadata: WorkerMetadata) -> worker::WorkerRegistration {
        let worker = without_recording(|| {
            let worker = Arc::new(WorkerControl {
                id: WorkerId::from_raw(next_id(&NEXT_WORKER_ID).get()).expect("worker identities are nonzero"),
                role: metadata.role,
                processor_index: metadata.processor_index,
                state: AtomicU8::new(WorkerState::Running.wire_value()),
                thread_id: AtomicU64::new(0),
            });
            let mut workers = lock(&self.control.workers);
            if self.control.state.load(Ordering::Acquire) == RuntimeState::Stopped.wire_value() {
                drop(workers);
                panic!("register workers before stopping their runtime");
            }
            workers.push(Arc::clone(&worker));
            worker
        });
        record_with(
            &self.control,
            EventTimestamp::now,
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

    pub(crate) fn capture(&self) -> RuntimeSnapshot {
        without_recording(|| snapshot_runtime(&self.control))
    }

    /// Marks the runtime as stopping and records the transition once.
    pub(crate) fn stopping(&self) {
        stopping_with(&self.control.state, || {
            record_with(
                &self.control,
                EventTimestamp::now,
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

    /// Retires workers and records the runtime's stop once.
    pub(crate) fn stopped(&self) {
        let at = EventTimestamp::now();
        if !self.retire_at(at) {
            return;
        }
        let workers = lock(&self.control.workers).clone();
        for worker in workers {
            worker::WorkerHandle::new(self.clone(), worker).stop();
        }
        record_with(
            &self.control,
            || at,
            None,
            EventKind::RuntimeStopped,
            0,
            0,
            0,
            0,
            self.control.lifecycle_backtraces,
        );
        lock(&REGISTRY).retain(|runtime| !Arc::ptr_eq(runtime, &self.control));
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

pub(crate) fn allocate_task_id() -> TaskId {
    TaskId::from_raw(next_id(&NEXT_TASK_ID).get()).expect("task identity counters start at one")
}

fn build_runtime(metadata: RuntimeBuilder) -> RuntimeHandle {
    let control = without_recording(|| {
        seismograph::snapshot::register_source(&SOURCE);
        let control = Arc::new(RuntimeControl {
            id: RuntimeId::from_raw(next_id(&NEXT_RUNTIME_ID).get()).expect("runtime identity counters start at one"),
            name: metadata.name,
            configured_workers: metadata.configured_workers,
            lifecycle_backtraces: metadata.lifecycle_backtraces,
            created_at: EventTimestamp::now(),
            retired_at: AtomicU64::new(0),
            state: AtomicU8::new(RuntimeState::Running.wire_value()),
            workers: Mutex::new(Vec::new()),
            task_locations: Mutex::new(BTreeMap::new()),
        });
        lock(&REGISTRY).push(Arc::clone(&control));
        control
    });
    record_with(
        &control,
        || control.created_at,
        None,
        EventKind::RuntimeCreated,
        0,
        0,
        u64::from(control.configured_workers),
        0,
        control.lifecycle_backtraces,
    );
    RuntimeHandle { control }
}

#[derive(Debug)]
pub(crate) struct RuntimeControl {
    pub(crate) id: RuntimeId,
    pub(crate) name: String,
    pub(crate) configured_workers: u32,
    pub(crate) lifecycle_backtraces: BacktraceCapture,
    pub(crate) created_at: EventTimestamp,
    pub(crate) retired_at: AtomicU64,
    pub(crate) state: AtomicU8,
    pub(crate) workers: Mutex<Vec<Arc<WorkerControl>>>,
    pub(crate) task_locations: Mutex<BTreeMap<TypeDescriptorId, &'static Location<'static>>>,
}

pub(crate) struct WorkerControl {
    pub(crate) id: WorkerId,
    pub(crate) role: WorkerRole,
    pub(crate) processor_index: Option<u32>,
    pub(crate) state: AtomicU8,
    pub(crate) thread_id: AtomicU64,
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
    pub(crate) fn snapshot(&self) -> Worker {
        Worker {
            id: self.id,
            role: self.role,
            state: WorkerState::from_wire_value(self.state.load(Ordering::Acquire)).expect("worker state stores enum values"),
            processor_index: self.processor_index,
            thread_id: NonZeroU64::new(self.thread_id.load(Ordering::Acquire))
                .map(|id| seismograph::recorder::thread::ThreadId::new(id.get())),
        }
    }
}

fn snapshot_runtime(runtime: &Arc<RuntimeControl>) -> RuntimeSnapshot {
    let state = RuntimeState::from_wire_value(runtime.state.load(Ordering::Acquire)).expect("runtime state stores enum values");
    RuntimeSnapshot {
        id: runtime.id,
        name: runtime.name.clone(),
        configured_workers: runtime.configured_workers,
        lifecycle_backtraces: runtime.lifecycle_backtraces,
        state,
        created_at: runtime.created_at,
        retired_at: (state == RuntimeState::Stopped).then(|| EventTimestamp::from_ticks(runtime.retired_at.load(Ordering::Acquire))),
        workers: lock(&runtime.workers).iter().map(|worker| worker.snapshot()).collect(),
        task_locations: lock(&runtime.task_locations)
            .iter()
            .map(|(type_descriptor, location)| TaskLocation {
                type_descriptor: *type_descriptor,
                file: location.file().into(),
                line: location.line(),
                column: location.column(),
            })
            .collect(),
    }
}

fn capture_source(_context: seismograph::snapshot::SnapshotContext<'_>) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let runtimes = lock(&REGISTRY).clone();
    let snapshot = Snapshot {
        runtimes: runtimes.iter().map(snapshot_runtime).collect(),
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
#[expect(
    clippy::too_many_arguments,
    reason = "runtime events have a fixed identity, timestamp, and numeric payload"
)]
pub(crate) fn record_with(
    control: &RuntimeControl,
    timestamp: impl FnOnce() -> EventTimestamp,
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
            timestamp(),
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

pub(crate) fn duration_nanos(end: EventTimestamp, start: EventTimestamp) -> u64 {
    end.ticks().saturating_sub(start.ticks())
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().expect("diagnostic state is never locked across user callbacks")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

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
        };
        assert_eq!(
            format!("{worker:?}"),
            format!("{} {{ id: WorkerId(13), role: Core, .. }}", std::any::type_name::<WorkerControl>())
        );
        assert_eq!(worker.snapshot().thread_id, None);
        worker.thread_id.store(19, Ordering::Relaxed);
        let captured = worker.snapshot();
        assert_eq!(captured.thread_id.unwrap().get(), 19);
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
            let runtime = Runtime::builder("retirement-boundaries").build();
            assert!(runtime.owner.handle.retire_at(EventTimestamp::from_ticks(at)));
            assert_eq!(runtime.owner.handle.control.retired_at.load(Ordering::Acquire), expected);
            assert_eq!(
                runtime.owner.handle.control.state.load(Ordering::Acquire),
                RuntimeState::Stopped.wire_value()
            );
            assert!(!runtime.owner.handle.retire_at(EventTimestamp::from_ticks(99)));
            assert_eq!(runtime.owner.handle.control.retired_at.load(Ordering::Acquire), expected);
        }
    }

    #[test]
    fn stopped_runtime_is_removed_from_the_live_source_registry() {
        let _test = test_lock();
        let runtime = Runtime::builder("retired-source").build();
        let control = Arc::clone(&runtime.owner.handle.control);

        drop(runtime);

        assert!(!lock(&REGISTRY).iter().any(|runtime| Arc::ptr_eq(runtime, &control)));
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
