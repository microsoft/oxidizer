// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph_runtime::WorkerOrigin;

pub(crate) fn current_origin() -> Option<WorkerOrigin> {
    crate::runtime::worker::asynchronous::LocalWorker::with_monitoring(|worker| {
        worker.map(seismograph_runtime::asynchronous::Worker::origin)
    })
    .or_else(|| {
        crate::runtime::worker::blocking::LocalWorker::with_monitoring(|worker| worker.map(seismograph_runtime::blocking::Worker::origin))
    })
}
