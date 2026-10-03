// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Recording-only task observations with a recording-independent terminal marker.
//!
//! Disabled wake/poll hooks skip this module. Registration still initializes its
//! inline storage, and terminal callbacks always publish retirement.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp};
use seismograph::recorder::runtime::WorkerId;
use seismograph::recorder::{RecordingObservation, RecordingSession, active_recording_session};

use crate::snapshot::{TaskActivity, TaskActivityState};
use crate::{TaskControl, runtime_record};

/// A non-blocking writer lock avoids both unbounded waker stalls and the unsound
/// multi-writer seqlock pattern. A failed writer invalidates the state with a
/// separate atomic revision: dropping a transition must never leave a plausible
/// but false Ready/Running state. Snapshots try once and return Unknown on conflict.
#[derive(Debug)]
pub(crate) struct Activity {
    data: Mutex<Data>,
    missed: AtomicU64,
    pub(crate) terminal: AtomicBool,
}

#[derive(Debug)]
struct Data {
    session: Option<RecordingSession>,
    revision: u64,
    updated_at: EventTimestamp,
    value: TaskActivity,
}

pub(crate) fn unknown(observed_at: EventTimestamp) -> TaskActivity {
    TaskActivity {
        observed_at,
        state: TaskActivityState::Unknown,
        ready_since: None,
        poll_started_at: None,
        poll_worker_id: None,
        queued_since: None,
    }
}

impl Activity {
    pub(crate) fn new(session: Option<RecordingSession>, at: EventTimestamp) -> Self {
        let mut value = unknown(at);
        if session.is_some() {
            value.state = TaskActivityState::Waiting;
        }
        Self {
            data: Mutex::new(Data {
                session,
                revision: 0,
                updated_at: at,
                value,
            }),
            missed: AtomicU64::new(0),
            terminal: AtomicBool::new(false),
        }
    }
    fn begin(&self, session: Option<RecordingSession>) -> Option<MutexGuard<'_, Data>> {
        let session = session?;
        if self.terminal.load(Ordering::Acquire) {
            return None;
        }
        let Ok(mut data) = self.data.try_lock() else {
            self.missed.fetch_add(1, Ordering::AcqRel);
            return None;
        };
        let revision = self.missed.load(Ordering::Acquire);
        if data.session != Some(session) || data.revision != revision {
            data.session = Some(session);
            data.revision = revision;
            data.value = unknown(data.updated_at);
        }
        if active_recording_session() != Some(session) || !seismograph::recorder::recording_enabled_for(EventClass::RuntimeTask) {
            self.missed.fetch_add(1, Ordering::AcqRel);
            return None;
        }
        Some(data)
    }

    fn validate(&self, data: &Data) -> bool {
        if active_recording_session() == data.session
            && seismograph::recorder::recording_enabled_for(EventClass::RuntimeTask)
            && !self.terminal.load(Ordering::Acquire)
        {
            true
        } else {
            self.missed.fetch_add(1, Ordering::AcqRel);
            false
        }
    }

    #[cold]
    pub(crate) fn woken(&self, task: &TaskControl) {
        let session = active_recording_session();
        let mut data = self.begin(session);
        // Serialize the existing clock/CAS with recorded poll starts. With
        // recording disabled those operations run without this synchronization.
        let at = task.wake_timestamp();
        if data.is_none() && session.is_some() {
            // Invalidate after the CAS as well: a failed writer may be delayed
            // while a successful poll attempts to recover coherent evidence.
            self.missed.fetch_add(1, Ordering::AcqRel);
        }
        let emit = if let Some(data) = &mut data {
            let first = data.value.ready_since.is_none();
            if first {
                data.value.ready_since = Some(at);
                if data.value.state == TaskActivityState::Waiting {
                    data.value.state = TaskActivityState::Ready;
                    data.value.queued_since = Some(at);
                }
            }
            data.updated_at = at;
            first && self.validate(data)
        } else {
            false
        };
        drop(data);
        if emit && let Some(session) = session {
            // A notifier is not a worker. The runtime ID avoids both an Arc
            // cycle and per-wake registry lookups.
            seismograph::recorder::record_in_session_classified(session, EventClass::RuntimeTask, || {
                Some(runtime_record(
                    at,
                    task.runtime_id,
                    None,
                    EventKind::TaskReady,
                    task.id.get(),
                    0,
                    0,
                    0,
                    BacktraceCapture::Never,
                ))
            });
        }
    }

    #[cold]
    pub(crate) fn poll_started(
        &self,
        task: &TaskControl,
        worker_id: WorkerId,
    ) -> (EventTimestamp, u64, Option<EventTimestamp>, Option<RecordingSession>) {
        let session = active_recording_session();
        let mut data = self.begin(session);
        let (at, ready) = task.poll_timestamps();
        if data.is_none() && session.is_some() {
            self.missed.fetch_add(1, Ordering::AcqRel);
        }
        let mut queued_since = None;
        if let Some(data) = &mut data {
            if data.value.state == TaskActivityState::Ready {
                queued_since = data.value.queued_since;
            }
            data.value = TaskActivity {
                observed_at: at,
                state: TaskActivityState::Running,
                ready_since: None,
                poll_started_at: Some(at),
                poll_worker_id: Some(worker_id),
                queued_since: None,
            };
            data.updated_at = at;
            match (self.validate(data), data.revision == self.missed.load(Ordering::Acquire)) {
                (true, true) => {}
                _ => queued_since = None,
            }
        }
        (at, ready, queued_since, session)
    }

    #[cold]
    pub(crate) fn poll_finished(&self, session: Option<RecordingSession>, started_at: EventTimestamp, finished_at: EventTimestamp) {
        if let Some(mut data) = self.begin(session) {
            if data.value.poll_started_at == Some(started_at) {
                data.value.poll_started_at = None;
                data.value.poll_worker_id = None;
                if let Some(ready) = data.value.ready_since {
                    data.value.state = TaskActivityState::Ready;
                    data.value.queued_since = Some(EventTimestamp::from_ticks(ready.ticks().max(finished_at.ticks())));
                } else {
                    data.value.state = TaskActivityState::Waiting;
                }
            } else {
                // A poll straddling enable/reset does not prove that no wake
                // occurred during its unobserved execution.
                data.value = unknown(finished_at);
            }
            data.updated_at = EventTimestamp::from_ticks(data.updated_at.ticks().max(finished_at.ticks()));
            self.validate(&data);
        }
    }

    pub(crate) fn snapshot(&self, observation: Option<RecordingObservation>) -> TaskActivity {
        let Some(observation) = observation else {
            return unknown(EventTimestamp::now());
        };
        let fallback = unknown(observation.observed_at);
        let Ok(data) = self.data.try_lock() else {
            return fallback;
        };
        if data.session != Some(observation.session) {
            return fallback;
        }
        if data.revision != self.missed.load(Ordering::Acquire) {
            return fallback;
        }
        if data.updated_at.ticks() > observation.observed_at.ticks() {
            return fallback;
        }
        if self.terminal.load(Ordering::Acquire) {
            return fallback;
        }
        if data.value.state == TaskActivityState::Unknown {
            return fallback;
        }
        TaskActivity {
            observed_at: observation.observed_at,
            ..data.value
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};
    use seismograph::recorder::{Configuration, EventBufferCapacity, RecordingPolicy, recording_observation};
    use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};

    use super::*;
    use crate::task::TaskHandle;
    use crate::worker::{WorkerMetadata, WorkerRole};
    use crate::{RuntimeMetadata, register_runtime};

    fn configure(enabled: bool) {
        seismograph::recorder(Configuration {
            runtime_tasks: RecordingPolicy {
                enabled,
                ..Default::default()
            },
            event_capacity_per_thread: EventBufferCapacity::new(64).unwrap(),
            ..Default::default()
        });
    }

    fn state(task: &TaskHandle) -> TaskActivity {
        task.task.activity.snapshot(recording_observation())
    }

    fn capture(disposition: EventBufferDisposition) -> seismograph::snapshot::DecodedSnapshot {
        let snapshot = seismograph::snapshot(SnapshotOptions {
            event_buffers: disposition,
        })
        .unwrap();
        seismograph::snapshot::decode(snapshot.as_bytes()).unwrap()
    }

    fn ready_events(task: &TaskHandle) -> Vec<seismograph::recorder::event::Event> {
        capture(EventBufferDisposition::Retain)
            .events
            .events
            .into_iter()
            .filter(|event| event.kind == EventKind::TaskReady && event.runtime().unwrap().subject_id == task.id().get())
            .collect()
    }

    #[test]
    fn stale_sessions_invalidate_begin_and_validation() {
        let _test = crate::tests::test_lock();
        configure(true);
        let current = active_recording_session().unwrap();
        let stale = RecordingSession::from_raw(current.get().wrapping_add(1).max(1)).unwrap();
        let activity = Activity::new(Some(stale), EventTimestamp::from_ticks(1));

        assert!(activity.begin(Some(stale)).is_none());
        let data = activity.data.lock().unwrap();
        assert!(!activity.validate(&data));
        assert_eq!(activity.missed.load(Ordering::Acquire), 2);
        drop(data);
        configure(false);
    }

    #[test]
    fn unrecorded_poll_start_still_tracks_current_task() {
        let _test = crate::tests::test_lock();
        configure(false);
        let runtime = register_runtime(RuntimeMetadata::new("unrecorded-poll", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task_id = TaskId::from_raw(7).unwrap();

        let poll = worker
            .handle()
            .task_poll_started_recorded(task_id, EventTimestamp::from_ticks(11), None, None);

        assert_eq!((poll.task_id, poll.started_at), (task_id, EventTimestamp::from_ticks(11)));
    }

    #[test]
    fn wakes_coalesce_across_threads_and_running_wakes_wait_for_poll_finish() {
        let _test = crate::tests::test_lock();
        configure(true);
        let runtime = register_runtime(RuntimeMetadata::new("activity", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        std::thread::scope(|scope| scope.spawn(|| task.woken()).join().unwrap());
        let ready = state(&task);
        task.woken();
        runtime.handle().task_enqueued(task.id(), Some(worker.id()));
        assert_eq!(state(&task).ready_since, ready.ready_since);
        assert_eq!(ready.state, TaskActivityState::Ready);
        assert_eq!(ready.queued_since, ready.ready_since);
        let events = ready_events(&task);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].timestamp, ready.ready_since.unwrap());
        assert_eq!(events[0].runtime().unwrap().worker_id, None);
        assert!(events[0].call_stack.is_empty());

        let poll = task.poll_started(&worker.handle());
        assert_eq!(state(&task).state, TaskActivityState::Running);
        task.woken();
        let running = state(&task);
        task.woken();
        assert_eq!(state(&task).ready_since, running.ready_since);
        assert_eq!(running.state, TaskActivityState::Running);
        assert_eq!(running.poll_started_at, Some(poll.started_at));
        assert_eq!(running.queued_since, None);
        task.poll_finished(&worker.handle(), poll);
        let queued = state(&task);
        assert_eq!(queued.state, TaskActivityState::Ready);
        assert_eq!(queued.ready_since, running.ready_since);
        assert!(queued.queued_since >= running.ready_since);
        assert_eq!(queued.poll_started_at, None);
        // The first capture released the exited notifier's ring.
        assert_eq!(ready_events(&task).len(), 1);

        let poll = task.poll_started(&worker.handle());
        task.poll_finished(&worker.handle(), poll);
        assert_eq!(state(&task).state, TaskActivityState::Waiting);
        runtime.handle().task_completed(task.id(), Some(worker.id()));
        task.woken();
        assert_eq!(ready_events(&task).len(), 1);
        assert_eq!(state(&task).state, TaskActivityState::Unknown);
        configure(false);
    }

    #[test]
    fn worker_finish_updates_recorded_task_activity_and_counters() {
        let _test = crate::tests::test_lock();
        configure(true);
        let runtime = register_runtime(RuntimeMetadata::new("worker-finish", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let worker = worker.handle();
        let other = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(2).unwrap(), None);
        let owners = Arc::strong_count(&task.task);

        let poll = task.poll_started(&worker);
        assert!(Arc::ptr_eq(poll.task.as_ref().unwrap(), &task.task));
        assert_eq!(Arc::strong_count(&task.task), owners + 1);
        assert_eq!(state(&task).state, TaskActivityState::Running);
        worker.task_poll_finished(poll);
        assert_eq!(Arc::strong_count(&task.task), owners);
        let waiting = state(&task);
        assert_eq!(waiting.state, TaskActivityState::Waiting);
        assert_eq!(waiting.poll_started_at, None);
        assert_eq!(waiting.poll_worker_id, None);
        assert_eq!(task.task.poll_count.load(Ordering::Relaxed), 1);
        assert_eq!(other.task.poll_count.load(Ordering::Relaxed), 0);

        let poll = task.poll_started(&worker);
        task.woken();
        worker.task_poll_finished(poll);
        let ready = state(&task);
        assert_eq!(ready.state, TaskActivityState::Ready);
        assert_eq!(ready.poll_started_at, None);
        assert_eq!(ready.poll_worker_id, None);
        assert_eq!(task.task.poll_count.load(Ordering::Relaxed), 2);
        configure(false);
    }

    #[test]
    fn disabled_hooks_leave_recording_state_and_event_buffers_untouched() {
        let _test = crate::tests::test_lock();
        configure(false);
        seismograph::recorder::clear_event_buffers().unwrap();
        let runtime = register_runtime(RuntimeMetadata::new("off", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        let before = format!("{:?}", task.task.activity);
        let owners = Arc::strong_count(&task.task);
        for _ in 0..10 {
            task.woken();
            let poll = task.poll_started(&worker.handle());
            assert!(poll.task.is_none());
            assert_eq!(Arc::strong_count(&task.task), owners);
            task.poll_finished(&worker.handle(), poll);
        }
        assert_eq!(format!("{:?}", task.task.activity), before);
        let captured = capture(EventBufferDisposition::Retain);
        assert!(captured.events.events.is_empty());
        assert!(captured.events.threads.is_empty());
        assert_eq!(task.task.poll_count.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn completed_queue_sample_survives_overwritten_self_wake_and_poll_finish() {
        let _test = crate::tests::test_lock();
        configure(true);
        let runtime = register_runtime(RuntimeMetadata::new("retained-queue-sample", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        let poll = task.poll_started(&worker.handle());
        task.woken();
        task.poll_finished(&worker.handle(), poll);
        let queued = state(&task);
        assert_eq!(queued.state, TaskActivityState::Ready);
        for _ in 0..70 {
            runtime.handle().task_enqueued(task.id(), None);
        }
        let poll = task.poll_started(&worker.handle());
        let snapshot = capture(EventBufferDisposition::Retain);
        let events = snapshot
            .events
            .events
            .iter()
            .filter(|event| event.runtime().is_some_and(|runtime| runtime.subject_id == task.id().get()))
            .collect::<Vec<_>>();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.kind, EventKind::TaskReady | EventKind::TaskPollFinished))
        );
        let start = events.iter().find(|event| event.kind == EventKind::TaskPollStarted).unwrap();
        assert_eq!(
            (start.runtime().unwrap().value_0, start.runtime().unwrap().value_1),
            (crate::duration_nanos(poll.started_at, queued.queued_since.unwrap()), 2)
        );
        assert!(start.runtime().unwrap().value_0 <= crate::duration_nanos(poll.started_at, queued.ready_since.unwrap()));
        task.poll_finished(&worker.handle(), poll);
        configure(false);
    }

    #[test]
    fn ongoing_poll_keeps_coherent_worker_after_migration_and_event_eviction() {
        let _test = crate::tests::test_lock();
        configure(true);
        let runtime = register_runtime(RuntimeMetadata::new("migrated-running-worker", 2));
        let first = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let second = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        let poll = task.poll_started(&first.handle());
        task.poll_finished(&first.handle(), poll);
        let poll = task.poll_started(&second.handle());
        // Reproduce stale independent source metadata while the coherent
        // activity describes a newer poll on the migrated worker.
        task.task.last_worker_id.store(first.id().get(), Ordering::Release);
        runtime
            .handle
            .control
            .workers
            .lock()
            .unwrap()
            .iter()
            .find(|worker| worker.id == first.id())
            .unwrap()
            .current_task
            .store(task.id().get(), Ordering::Release);
        for _ in 0..70 {
            runtime.handle().task_enqueued(task.id(), None);
        }
        let snapshot = capture(EventBufferDisposition::Retain);
        assert!(!snapshot.events.events.iter().any(|event| event.kind == EventKind::TaskPollStarted));
        let source = snapshot
            .sources
            .iter()
            .find(|source| source.id == crate::snapshot::source::ID)
            .unwrap();
        let source = crate::snapshot::decode(&source.data).unwrap();
        let runtime_source = source.runtimes.iter().find(|item| item.id == runtime.id()).unwrap();
        let task_source = runtime_source.tasks.iter().find(|item| item.id == task.id()).unwrap();
        let activity = task_source.activity.unwrap();
        assert_eq!(task_source.last_worker_id, Some(first.id()));
        assert_eq!(
            (activity.state, activity.poll_worker_id, activity.poll_started_at),
            (TaskActivityState::Running, Some(second.id()), Some(poll.started_at))
        );
        task.poll_finished(&second.handle(), poll);
        assert_eq!(state(&task).poll_worker_id, None);
        configure(false);
    }

    #[test]
    fn unobserved_running_state_cannot_emit_a_pure_queue_sample() {
        let _test = crate::tests::test_lock();
        configure(false);
        let runtime = register_runtime(RuntimeMetadata::new("unknown-queue-sample", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        configure(true);
        task.woken();
        assert_eq!(state(&task).state, TaskActivityState::Unknown);
        let poll = task.poll_started(&worker.handle());
        let snapshot = capture(EventBufferDisposition::Retain);
        let start = snapshot
            .events
            .events
            .iter()
            .find(|event| event.kind == EventKind::TaskPollStarted)
            .unwrap();
        assert_eq!((start.runtime().unwrap().value_0, start.runtime().unwrap().value_1), (0, 0));
        task.poll_finished(&worker.handle(), poll);
        configure(false);
    }

    #[test]
    fn legacy_poll_helper_preserves_raw_wake_sample_discriminator() {
        let _test = crate::tests::test_lock();
        configure(true);
        let runtime = register_runtime(RuntimeMetadata::new("legacy-wake-sample", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().task_spawned(TypeDescriptorId::from_raw(1).unwrap(), None);
        let ready = EventTimestamp::from_ticks(10);
        let poll = worker
            .handle()
            .task_poll_started_at(task, EventTimestamp::from_ticks(110), Some(ready));
        let snapshot = capture(EventBufferDisposition::Retain);
        let start = snapshot
            .events
            .events
            .iter()
            .find(|event| event.kind == EventKind::TaskPollStarted)
            .unwrap();
        assert_eq!((start.runtime().unwrap().value_0, start.runtime().unwrap().value_1), (100, 1));
        worker.handle().task_poll_finished(poll);
        configure(false);
    }

    #[test]
    fn enable_reset_clear_and_stop_do_not_reuse_old_ready_ages() {
        let _test = crate::tests::test_lock();
        configure(false);
        let runtime = register_runtime(RuntimeMetadata::new("generations", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        task.woken();
        configure(true);
        assert_eq!(state(&task).state, TaskActivityState::Unknown);
        let poll = task.poll_started(&worker.handle());
        let events = capture(EventBufferDisposition::Retain);
        let start = events
            .events
            .events
            .iter()
            .find(|event| event.kind == EventKind::TaskPollStarted)
            .unwrap();
        assert_eq!(start.runtime().unwrap().value_1, 0);
        task.poll_finished(&worker.handle(), poll);
        task.woken();
        assert_eq!(state(&task).state, TaskActivityState::Ready);
        let old_session = active_recording_session();
        seismograph::recorder::clear_event_buffers().unwrap();
        assert_ne!(old_session, active_recording_session());
        assert_eq!(state(&task).state, TaskActivityState::Unknown);
        let poll = task.poll_started(&worker.handle());
        task.poll_finished(&worker.handle(), poll);
        task.woken();
        let stopped = capture(EventBufferDisposition::Stop);
        let source = stopped
            .sources
            .iter()
            .find(|source| source.id == crate::snapshot::source::ID)
            .unwrap();
        let decoded = crate::snapshot::decode(&source.data).unwrap();
        let stopped_activity = decoded.runtimes.iter().find(|item| item.id == runtime.id()).unwrap().tasks[0]
            .activity
            .unwrap();
        assert_eq!(stopped_activity.state, TaskActivityState::Ready);
        let frozen = state(&task);
        let poll = task.poll_started(&worker.handle());
        task.woken();
        task.poll_finished(&worker.handle(), poll);
        assert_eq!(state(&task), frozen);
        assert_eq!(frozen, stopped_activity);
        seismograph::recorder::clear_event_buffers().unwrap();
        assert_eq!(state(&task).state, TaskActivityState::Unknown);
        configure(true);
        assert_eq!(state(&task).state, TaskActivityState::Unknown);
        let poll = task.poll_started(&worker.handle());
        capture(EventBufferDisposition::Release);
        task.poll_finished(&worker.handle(), poll);
        assert_eq!(state(&task).state, TaskActivityState::Unknown);
        configure(false);
    }

    #[test]
    fn contended_writers_do_not_wait_and_invalidate_incomplete_state() {
        let _test = crate::tests::test_lock();
        configure(true);
        let runtime = register_runtime(RuntimeMetadata::new("contention", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        task.woken();
        let guard = task.task.activity.data.lock().unwrap();
        std::thread::scope(|scope| scope.spawn(|| task.woken()).join().unwrap());
        assert_eq!(state(&task).state, TaskActivityState::Unknown);
        drop(guard);
        assert_eq!(state(&task).state, TaskActivityState::Unknown);
        let poll = task.poll_started(&worker.handle());
        assert_eq!(state(&task).state, TaskActivityState::Running);
        task.poll_finished(&worker.handle(), poll);
        configure(false);
    }

    #[test]
    fn contended_poll_start_invalidates_unrecorded_activity() {
        let _test = crate::tests::test_lock();
        configure(true);
        let runtime = register_runtime(RuntimeMetadata::new("poll-contention", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        task.woken();
        let guard = task.task.activity.data.lock().unwrap();

        let (_, _, queued_since, session) = std::thread::scope(|scope| {
            scope
                .spawn(|| task.task.activity.poll_started(&task.task, worker.id()))
                .join()
                .unwrap()
        });

        assert_eq!((queued_since, session), (None, active_recording_session()));
        assert_eq!(task.task.activity.missed.load(Ordering::Acquire), 2);
        drop(guard);
        assert_eq!(state(&task).state, TaskActivityState::Unknown);
        configure(false);
    }

    #[test]
    fn repeated_disable_keeps_the_first_observation_boundary_and_terminal_wakes_stay_silent() {
        let _test = crate::tests::test_lock();
        configure(true);
        let runtime = register_runtime(RuntimeMetadata::new("disabled-boundary", 1));
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        task.woken();
        configure(false);
        let stopped = state(&task);
        task.woken();
        configure(false);
        assert_eq!(state(&task), stopped);
        assert_eq!(stopped.state, TaskActivityState::Ready);
        configure(true);
        runtime.handle().task_canceled(task.id(), None);
        task.woken();
        assert!(ready_events(&task).is_empty());
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        runtime.handle().task_panicked(task.id(), None);
        task.woken();
        assert!(ready_events(&task).is_empty());
        configure(false);
    }

    #[test]
    fn poll_finish_after_stop_invalidates_the_frozen_running_state() {
        let _test = crate::tests::test_lock();
        configure(true);
        let runtime = register_runtime(RuntimeMetadata::new("stop-running", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        let poll = task.poll_started(&worker.handle());
        task.woken();
        capture(EventBufferDisposition::Stop);
        let frozen = state(&task);
        task.poll_finished(&worker.handle(), poll);
        task.woken();
        assert_eq!(state(&task).state, TaskActivityState::Unknown);
        assert_eq!(frozen.state, TaskActivityState::Running);
        assert!(frozen.ready_since.is_some() && frozen.poll_started_at.is_some());
        assert_eq!(frozen.queued_since, None);
        configure(false);
    }

    #[test]
    fn identity_only_legacy_hooks_do_not_claim_waiting_while_running() {
        let _test = crate::tests::test_lock();
        configure(true);
        let runtime = register_runtime(RuntimeMetadata::new("legacy-hooks", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().task_spawned(TypeDescriptorId::from_raw(1).unwrap(), None);
        runtime.handle().task_enqueued(task, None);
        let poll = worker.handle().task_poll_started(task);
        let snapshot = capture(EventBufferDisposition::Retain);
        let source = snapshot
            .sources
            .iter()
            .find(|source| source.id == crate::snapshot::source::ID)
            .unwrap();
        let source = crate::snapshot::decode(&source.data).unwrap();
        let activity = source.runtimes.iter().find(|item| item.id == runtime.id()).unwrap().tasks[0]
            .activity
            .unwrap();
        assert_eq!(activity.state, TaskActivityState::Unknown);
        worker.handle().task_poll_finished(poll);
        configure(false);
    }

    #[test]
    fn identity_only_registration_initializes_unknown_without_invalidation() {
        let _test = crate::tests::test_lock();
        for enabled in [false, true] {
            configure(enabled);
            let runtime = register_runtime(RuntimeMetadata::new("legacy-initialization", 1));
            let task_id = runtime.handle().task_spawned(TypeDescriptorId::from_raw(1).unwrap(), None);
            let tasks = runtime.handle.control.tasks.lock().unwrap();
            let task = tasks.iter().find(|task| task.id == task_id).unwrap();
            let data = task.activity.data.lock().unwrap();
            assert_eq!(
                (
                    data.session,
                    data.revision,
                    data.value.state,
                    task.activity.missed.load(Ordering::Acquire)
                ),
                (None, 0, TaskActivityState::Unknown, 0)
            );
        }
        configure(false);
    }

    #[test]
    fn retirement_while_disabled_prevents_phantom_wakes_after_reenable() {
        let _test = crate::tests::test_lock();
        let finishers: [fn(&crate::RuntimeHandle, TaskId, Option<WorkerId>); 3] = [
            crate::RuntimeHandle::task_completed,
            crate::RuntimeHandle::task_canceled,
            crate::RuntimeHandle::task_panicked,
        ];
        for finish in finishers {
            configure(true);
            let runtime = register_runtime(RuntimeMetadata::new("off-retirement", 1));
            let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
            task.woken();
            configure(false);
            finish(&runtime.handle(), task.id(), None);
            assert!(task.task.activity.terminal.load(Ordering::Acquire));
            configure(true);
            task.woken();
            assert!(ready_events(&task).is_empty());
            assert_eq!(state(&task).state, TaskActivityState::Unknown);
        }
        configure(false);
    }

    #[test]
    fn outstanding_ten_minute_ready_and_long_poll_use_observation_clock() {
        let session = RecordingSession::from_raw(123).unwrap();
        let start = EventTimestamp::from_ticks(10);
        let observed_at = EventTimestamp::from_ticks(600_000_000_010);
        let activity = Activity::new(Some(session), start);
        let observation = Some(RecordingObservation { session, observed_at });
        for state in [TaskActivityState::Ready, TaskActivityState::Running] {
            {
                let mut data = activity.data.lock().unwrap();
                data.value.state = state;
                data.value.ready_since = (state == TaskActivityState::Ready).then_some(start);
                data.value.queued_since = data.value.ready_since;
                data.value.poll_started_at = (state == TaskActivityState::Running).then_some(start);
            }
            let snapshot = activity.snapshot(observation);
            let since = snapshot.queued_since.or(snapshot.poll_started_at).unwrap();
            assert_eq!(snapshot.observed_at.duration_since(since), std::time::Duration::from_mins(10));
        }
    }

    #[test]
    fn snapshot_accepts_an_observation_at_the_exact_update_boundary() {
        let session = RecordingSession::from_raw(123).unwrap();
        let at = EventTimestamp::from_ticks(10);
        let activity = Activity::new(Some(session), at);
        assert_eq!(
            activity.snapshot(Some(RecordingObservation { session, observed_at: at })).state,
            TaskActivityState::Waiting
        );
        assert_eq!(
            activity
                .snapshot(Some(RecordingObservation {
                    session,
                    observed_at: EventTimestamp::from_ticks(9),
                }))
                .state,
            TaskActivityState::Unknown
        );
    }

    #[test]
    fn terminal_activity_snapshot_is_unknown() {
        let session = RecordingSession::from_raw(123).unwrap();
        let observed_at = EventTimestamp::from_ticks(10);
        let activity = Activity::new(Some(session), observed_at);
        activity.terminal.store(true, Ordering::Release);

        assert_eq!(
            activity.snapshot(Some(RecordingObservation { session, observed_at })).state,
            TaskActivityState::Unknown
        );
    }

    #[test]
    fn concurrent_snapshots_never_mix_ready_and_running_fields() {
        let at = EventTimestamp::from_ticks(1);
        assert_snapshot_consistent(
            TaskActivity {
                observed_at: at,
                state: TaskActivityState::Ready,
                ready_since: Some(at),
                poll_started_at: None,
                poll_worker_id: None,
                queued_since: Some(at),
            },
            WorkerId::from_raw(1).unwrap(),
        );
        let _test = crate::tests::test_lock();
        configure(true);
        let runtime = register_runtime(RuntimeMetadata::new("concurrent-state", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let task = runtime.handle().register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..200 {
                    let poll = task.poll_started(&worker.handle());
                    task.poll_finished(&worker.handle(), poll);
                }
            });
            scope.spawn(|| {
                for _ in 0..200 {
                    task.woken();
                }
            });
            for _ in 0..400 {
                assert_snapshot_consistent(state(&task), worker.id());
            }
        });
        configure(false);
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // OS scheduling determines which valid concurrent states this assertion helper observes.
    fn assert_snapshot_consistent(value: TaskActivity, worker_id: WorkerId) {
        match value.state {
            TaskActivityState::Unknown | TaskActivityState::Waiting => {
                assert_eq!((value.ready_since, value.poll_started_at, value.queued_since), (None, None, None));
                assert_eq!(value.poll_worker_id, None);
            }
            TaskActivityState::Ready => {
                assert!(value.ready_since.is_some() && value.queued_since >= value.ready_since);
                assert_eq!(value.poll_started_at, None);
                assert_eq!(value.poll_worker_id, None);
            }
            TaskActivityState::Running => {
                assert!(value.poll_started_at.is_some());
                assert_eq!(value.poll_worker_id, Some(worker_id));
                assert_eq!(value.queued_since, None);
            }
        }
        assert!(value.ready_since.is_none_or(|at| at <= value.observed_at));
        assert!(value.poll_started_at.is_none_or(|at| at <= value.observed_at));
    }
}
