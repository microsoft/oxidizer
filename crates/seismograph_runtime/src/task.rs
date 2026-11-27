// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native asynchronous task inspection requests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use crate::collection;
use crate::runtime::{WorkerControl, lock};
use crate::snapshot::{Task, TaskCollection, TaskMetrics};

/// An owned request that a provider moves to its worker and answers once.
#[derive(Debug)]
pub struct TaskRequest {
    pub(crate) worker: Arc<WorkerControl>,
    pub(crate) sender: mpsc::SyncSender<collection::Batch>,
    pub(crate) canceled: Arc<AtomicBool>,
    pub(crate) limit: usize,
    pub(crate) window: u64,
    pub(crate) answered: bool,
}

impl TaskRequest {
    /// Maximum native tasks to inspect.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Whether the diagnostic caller stopped waiting for this request.
    #[must_use]
    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }

    /// Sends owned task descriptions without waiting for the diagnostic caller.
    pub fn respond(mut self, mut tasks: Vec<Task>, mut collection: TaskCollection) {
        if self.is_canceled() {
            return;
        }
        if tasks.len() > self.limit {
            tasks.truncate(self.limit);
            collection = TaskCollection::Truncated;
        }
        let measurements = lock(&self.worker.measurements);
        for task in &mut tasks {
            task.last_worker_id = Some(self.worker.id);
            task.metrics = TaskMetrics::default();
            task.metrics_scope = crate::snapshot::TaskMetricsScope::Uncollected;
            if self.window != 0
                && measurements.window == self.window
                && let Some(measured) = measurements.tasks.get(&task.id)
            {
                task.metrics = measured.metrics;
                task.metrics_scope = crate::snapshot::TaskMetricsScope::PollsSince(measured.since);
            }
        }
        drop(measurements);
        self.answered = true;
        self.send(collection::Batch {
            worker: self.worker.id,
            tasks,
            collection,
        });
    }

    fn send(&self, batch: collection::Batch) {
        match self.sender.try_send(batch) {
            Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => {}
            Err(mpsc::TrySendError::Full(_)) => unreachable!("one response per worker fits the collection channel"),
        }
    }
}

impl Drop for TaskRequest {
    fn drop(&mut self) {
        if !self.answered && !self.is_canceled() {
            self.send(collection::Batch {
                worker: self.worker.id,
                tasks: Vec::new(),
                collection: TaskCollection::Failed,
            });
        }
        self.worker.request_pending.store(false, Ordering::Release);
    }
}
