// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io;

use data_privacy::classified;
use observed::event;

static WORKER_METADATA: data_privacy::DataClass = data_privacy::DataClass::new("health", "WorkerMetadata");
static WORKER_ERROR_MESSAGE: data_privacy::DataClass = data_privacy::DataClass::new("health", "WorkerErrorMessage");

#[classified(WORKER_METADATA)]
#[derive(Clone, Copy)]
pub(crate) struct WorkerName(pub(crate) &'static str);

#[classified(WORKER_ERROR_MESSAGE)]
#[derive(Clone)]
pub(crate) struct WorkerErrorMessage(pub(crate) String);

impl From<&io::Error> for WorkerErrorMessage {
    fn from(error: &io::Error) -> Self {
        Self(error.to_string())
    }
}

#[event("health.worker.start_failed")]
#[error("health worker failed to start")]
pub(crate) struct WorkerStartFailed {
    #[dimension(log = "worker.name")]
    pub(crate) worker: WorkerName,
    #[dimension(log = "error.message")]
    pub(crate) error: WorkerErrorMessage,
}

#[event("health.worker.terminated")]
#[error("health worker terminated unexpectedly")]
pub(crate) struct WorkerTerminated {
    #[dimension(log = "worker.name")]
    pub(crate) worker: WorkerName,
}
