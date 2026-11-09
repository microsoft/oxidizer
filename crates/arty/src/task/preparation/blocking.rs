// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};

use arty_executor::{ShutdownCause, TaskCompletion};

use crate::runtime::hook::{BlockingPanicHook, PanicAction};
use crate::task::handle::{JoinHandle, JoinHandlePool, Sender};
use crate::task::preparation::discard_panicking_drop;

pub(crate) fn prepare_blocking<F, R, P>(
    result_pool: &JoinHandlePool,
    body: F,
    panic_hook: P,
) -> (impl FnOnce() + Send + 'static, JoinHandle<R>)
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
    P: Into<BlockingPanicHook>,
{
    let (handle, result_tx) = result_pool.handle();
    let task = BlockingTask {
        pending: Some((body, result_tx)),
        panic_hook: panic_hook.into(),
    };
    (move || task.run(), handle)
}

struct BlockingTask<F, R: Send + 'static> {
    pending: Option<(F, Sender<R>)>,
    panic_hook: BlockingPanicHook,
}

impl<F, R> BlockingTask<F, R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    fn run(mut self) {
        let (body, result_tx) = self.pending.take().expect("a blocking task is consumed exactly once");
        match catch_unwind(AssertUnwindSafe(body)) {
            Ok(result) => {
                result_tx.completed(result);
            }
            Err(panic) => {
                if self.panic_hook.notify() == PanicAction::Shutdown {
                    result_tx.shutdown().begin(ShutdownCause::panic());
                }
                result_tx.panicked_after_hook(panic);
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
