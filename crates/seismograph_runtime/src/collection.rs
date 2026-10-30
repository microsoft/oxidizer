// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use seismograph::recorder::event::EventTimestamp;
use seismograph::recorder::runtime::{TaskId, WorkerId};

use crate::snapshot::{CaptureOptions, Counters, Runtime, RuntimeState, Task, TaskCollection, TaskMetrics, WorkerState};
use crate::task::TaskRequest;
use crate::{RuntimeControl, WorkerControl, lock};

const MEASUREMENT_TASK_LIMIT: usize = 4096;

#[derive(Debug, Default)]
pub(crate) struct Timing {
    next: u64,
    users: usize,
}

#[derive(Debug, Default)]
pub(crate) struct Measurements {
    pub(crate) window: u64,
    pub(crate) tasks: HashMap<TaskId, Measured>,
}

#[derive(Debug)]
pub(crate) struct Measured {
    pub(crate) since: EventTimestamp,
    pub(crate) metrics: TaskMetrics,
}

impl Measurements {
    pub(crate) fn add(&mut self, window: u64, task: TaskId, start: EventTimestamp, elapsed: u64) {
        if self.window != window {
            self.window = window;
            self.tasks.clear();
        }
        if self.tasks.len() == MEASUREMENT_TASK_LIMIT && !self.tasks.contains_key(&task) {
            return;
        }
        let measured = self.tasks.entry(task).or_insert_with(|| Measured {
            since: start,
            metrics: TaskMetrics::default(),
        });
        measured.metrics.poll_count = measured.metrics.poll_count.saturating_add(1);
        measured.metrics.poll_duration_nanos = measured.metrics.poll_duration_nanos.saturating_add(elapsed);
        measured.metrics.max_poll_duration_nanos = measured.metrics.max_poll_duration_nanos.max(elapsed);
    }
}

#[derive(Debug)]
pub(crate) struct Batch {
    pub(crate) worker: WorkerId,
    pub(crate) tasks: Vec<Task>,
    pub(crate) collection: TaskCollection,
}

struct Observation<'a> {
    runtime: &'a RuntimeControl,
    timed: bool,
}

impl<'a> Observation<'a> {
    fn new(runtime: &'a RuntimeControl, timed: bool) -> Self {
        runtime.requests.fetch_add(1, Ordering::Relaxed);
        if timed {
            let mut timing = lock(&runtime.timing);
            if timing.users == 0 {
                timing.next = timing
                    .next
                    .checked_add(1)
                    .expect("a process cannot exhaust u64 observation windows");
            }
            timing.users += 1;
            runtime.timing_window.store(timing.next, Ordering::Release);
        }
        Self { runtime, timed }
    }
}

impl Drop for Observation<'_> {
    fn drop(&mut self) {
        if self.timed {
            let mut timing = lock(&self.runtime.timing);
            timing.users -= 1;
            if timing.users == 0 {
                self.runtime.timing_window.store(0, Ordering::Release);
                for worker in lock(&self.runtime.workers).iter() {
                    lock(&worker.measurements).tasks.clear();
                }
            }
        }
        self.runtime.requests.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) fn capture(runtime: &Arc<RuntimeControl>, options: CaptureOptions) -> Runtime {
    let _observation = Observation::new(runtime, !options.measure_for.is_zero());
    if !options.measure_for.is_zero() {
        std::thread::sleep(options.measure_for);
    }
    let window = runtime.timing_window.load(Ordering::Acquire);
    let workers = lock(&runtime.workers).clone();
    let (sender, receiver) = mpsc::sync_channel(workers.len().max(1));
    let canceled = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + options.timeout;
    let mut pending = HashSet::new();
    let mut batches = HashMap::new();
    for worker in &workers {
        if let Some(batch) = dispatch(worker, &sender, &canceled, options.limit_per_worker, window) {
            batches.insert(worker.id, batch);
        } else {
            pending.insert(worker.id);
        }
    }
    drop(sender);
    let unavailable = loop {
        if pending.is_empty() {
            break TaskCollection::Complete;
        }
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(batch) => {
                pending.remove(&batch.worker);
                batches.insert(batch.worker, batch);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break TaskCollection::TimedOut,
            Err(mpsc::RecvTimeoutError::Disconnected) => break TaskCollection::Failed,
        }
    };
    canceled.store(true, Ordering::Release);
    for id in pending {
        batches.insert(
            id,
            Batch {
                worker: id,
                tasks: Vec::new(),
                collection: unavailable,
            },
        );
    }
    let mut tasks = Vec::new();
    let workers = workers
        .iter()
        .map(|worker| {
            let batch = batches
                .remove(&worker.id)
                .expect("every worker has a response or explicit availability");
            tasks.extend(batch.tasks);
            worker.snapshot(batch.collection)
        })
        .collect();
    tasks.sort_unstable_by_key(|task| task.id);
    let state = RuntimeState::from_wire_value(runtime.state.load(Ordering::Acquire)).expect("runtime state stores enum values");
    Runtime {
        id: runtime.id,
        name: runtime.name.clone(),
        configured_workers: runtime.configured_workers,
        lifecycle_backtraces: runtime.lifecycle_backtraces,
        state,
        created_at: runtime.created_at,
        retired_at: (state == RuntimeState::Stopped).then(|| EventTimestamp::from_ticks(runtime.retired_at.load(Ordering::Acquire))),
        counters: Counters::default(),
        counters_scope: crate::snapshot::CountersScope::Uncollected,
        workers,
        tasks,
    }
}

fn dispatch(
    worker: &Arc<WorkerControl>,
    sender: &mpsc::SyncSender<Batch>,
    canceled: &Arc<AtomicBool>,
    limit: usize,
    window: u64,
) -> Option<Batch> {
    let provider = lock(&worker.provider).clone();
    let unavailable = if worker.state.load(Ordering::Acquire) == WorkerState::Stopped.wire_value() {
        Some(TaskCollection::Stopped)
    } else if provider.is_none() {
        Some(TaskCollection::Unavailable)
    } else if worker.request_pending.swap(true, Ordering::AcqRel) {
        Some(TaskCollection::Busy)
    } else {
        None
    };
    if let Some(collection) = unavailable {
        return Some(Batch {
            worker: worker.id,
            tasks: Vec::new(),
            collection,
        });
    }
    let request = TaskRequest {
        worker: Arc::clone(worker),
        sender: sender.clone(),
        canceled: Arc::clone(canceled),
        limit,
        window,
    };
    let provider = provider.expect("only workers with a provider reach request dispatch");
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| provider(request))).is_err() {
        return Some(Batch {
            worker: worker.id,
            tasks: Vec::new(),
            collection: TaskCollection::Failed,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{TaskActivityState, TaskMetricsScope};
    use crate::worker::{WorkerMetadata, WorkerRole};
    use crate::{RuntimeMetadata, allocate_task_id, allocate_type_descriptor_id, register_runtime};

    #[test]
    fn a_poll_from_a_finished_window_cannot_enter_the_next_window() {
        let runtime = register_runtime(RuntimeMetadata::new("measurement-window", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let handle = worker.handle();
        let id = allocate_task_id();
        let descriptor = allocate_type_descriptor_id();
        handle
            .register_task_provider(move |request| {
                request.respond(
                    vec![Task::new(id, descriptor, TaskActivityState::Waiting)],
                    TaskCollection::Complete,
                );
            })
            .unwrap();
        let first = Observation::new(&runtime.handle.control, true);
        let stale_poll = handle.task_poll_started(id);
        drop(first);
        let second = Observation::new(&runtime.handle.control, true);
        handle.task_poll_finished(stale_poll);
        let stale = runtime.handle().capture(CaptureOptions::default()).unwrap();

        let poll = handle.task_poll_started(id);
        handle.task_poll_finished(poll);
        let current = runtime.handle().capture(CaptureOptions::default()).unwrap();
        drop(second);

        assert_eq!(
            (
                stale.tasks[0].metrics_scope,
                current.tasks[0].metrics.poll_count,
                handle.is_observed()
            ),
            (TaskMetricsScope::Uncollected, 1, false)
        );
    }
}
