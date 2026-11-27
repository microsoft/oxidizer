// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};

use arty_executor::TaskCompletion;

use crate::runtime::monitoring::BlockingJob;
use crate::task::handle::{JoinHandle, JoinHandlePool, Sender};
use crate::task::preparation::discard_panicking_drop;

pub(crate) fn prepare_blocking<F, R>(
    result_pool: &JoinHandlePool,
    body: F,
    monitoring: Option<seismograph_runtime::blocking::TaskHandle>,
) -> (BlockingJob, JoinHandle<R>)
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (handle, result_tx) = result_pool.handle();
    let task = BlockingTask {
        pending: Some((body, result_tx)),
    };
    (BlockingJob::prepare(task, monitoring), handle)
}

pub(crate) struct BlockingTask<F, R: Send + 'static> {
    pub(crate) pending: Option<(F, Sender<R>)>,
}

impl<F, R> BlockingTask<F, R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    pub(crate) fn run(mut self, finished: impl FnOnce(seismograph_runtime::TaskOutcome)) {
        let (body, result_tx) = self.pending.take().expect("a blocking task is consumed exactly once");
        match catch_unwind(AssertUnwindSafe(body)) {
            Ok(result) => {
                finished(seismograph_runtime::TaskOutcome::Completed);
                result_tx.completed(result);
            }
            Err(panic) => {
                finished(seismograph_runtime::TaskOutcome::Panicked);
                result_tx.panicked(panic, None);
            }
        }
    }
}

impl<F, R: Send + 'static> Drop for BlockingTask<F, R> {
    fn drop(&mut self) {
        let Some((body, result_tx)) = self.pending.take() else {
            return;
        };
        discard_panicking_drop(body);
        drop(result_tx);
    }
}
