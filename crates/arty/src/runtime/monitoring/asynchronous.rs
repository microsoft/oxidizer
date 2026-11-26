// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::cell::Cell;

use arty_executor::{Executor, InspectionError, InspectionStatus, PollObserver};
use seismograph_runtime::TaskIdentity;
use seismograph_runtime::asynchronous::TaskRequest;
use seismograph_runtime::snapshot::TaskCollection;

#[derive(Debug, Default)]
pub(crate) struct InspectionObserver {
    failed: Cell<bool>,
}

impl InspectionObserver {
    pub(crate) fn collect(&self, executor: &mut Executor, request: TaskRequest) {
        if request.is_canceled() {
            return;
        }
        let mut tasks = Vec::new();
        let status = collection_status(executor.inspect(request.limit(), |task: &seismograph_runtime::snapshot::Task| {
            tasks.push(task.clone());
        }));
        let status = if self.failed.replace(false) {
            TaskCollection::Failed
        } else {
            status
        };
        request.respond(tasks, status);
    }

    fn inspection_failed(&self) {
        self.failed.set(true);
    }
}

impl PollObserver for InspectionObserver {
    fn poll_started(&self, description: &dyn Any) {
        if !description.is::<TaskIdentity>() {
            self.inspection_failed();
        }
    }

    fn poll_finished(&self) {}

    fn inspection_failed(&self) {
        self.inspection_failed();
    }
}

fn collection_status(result: Result<InspectionStatus, InspectionError>) -> TaskCollection {
    match result {
        Ok(InspectionStatus::Complete) => TaskCollection::Complete,
        Ok(InspectionStatus::Truncated) => TaskCollection::Truncated,
        Err(InspectionError::Busy) => TaskCollection::Busy,
        Err(InspectionError::InvalidLimit | InspectionError::Panicked) => TaskCollection::Failed,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn inspection_outcomes_preserve_completeness_and_failure() {
        assert_eq!(
            [
                Ok(InspectionStatus::Complete),
                Ok(InspectionStatus::Truncated),
                Err(InspectionError::Busy),
                Err(InspectionError::InvalidLimit),
                Err(InspectionError::Panicked),
            ]
            .map(collection_status),
            [
                TaskCollection::Complete,
                TaskCollection::Truncated,
                TaskCollection::Busy,
                TaskCollection::Failed,
                TaskCollection::Failed,
            ]
        );
    }
}
