// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};

use arty_executor::{ShutdownCause, TaskCompletion};

use crate::runtime::ShutdownStart;
use crate::runtime::worker::blocking::PanicPolicy;
use crate::task::handle::{JoinHandle, JoinHandlePool, Sender};
use crate::task::preparation::discard_panicking_drop;

#[cfg(any(test, not(feature = "monitor")))]
pub(crate) fn prepare_blocking<F, R>(
    result_pool: &JoinHandlePool,
    body: F,
    panic_policy: PanicPolicy,
) -> (impl FnOnce() + Send + 'static, JoinHandle<R>)
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    prepare_blocking_inner(
        result_pool,
        body,
        panic_policy,
        #[cfg(feature = "monitor")]
        None,
    )
}

#[cfg(feature = "monitor")]
pub(crate) fn prepare_blocking_with_info<F, R>(
    result_pool: &JoinHandlePool,
    body: F,
    panic_policy: PanicPolicy,
    info: Option<std::sync::Arc<crate::runtime::monitoring::TaskInfo>>,
) -> (impl FnOnce() + Send + 'static, JoinHandle<R>)
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    prepare_blocking_inner(result_pool, body, panic_policy, info)
}

fn prepare_blocking_inner<F, R>(
    result_pool: &JoinHandlePool,
    body: F,
    panic_policy: PanicPolicy,
    #[cfg(feature = "monitor")] info: Option<std::sync::Arc<crate::runtime::monitoring::TaskInfo>>,
) -> (impl FnOnce() + Send + 'static, JoinHandle<R>)
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (handle, result_tx) = result_pool.handle();
    let task = BlockingTask {
        pending: Some((body, result_tx)),
        panic_policy,
        #[cfg(feature = "monitor")]
        info,
    };
    (move || task.run(), handle)
}

struct BlockingTask<F, R: Send + 'static> {
    pending: Option<(F, Sender<R>)>,
    panic_policy: PanicPolicy,
    #[cfg(feature = "monitor")]
    info: Option<std::sync::Arc<crate::runtime::monitoring::TaskInfo>>,
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
                #[cfg(feature = "monitor")]
                if let Some(info) = &self.info {
                    info.completed();
                }
                result_tx.completed(result);
            }
            Err(panic) => {
                #[cfg(feature = "monitor")]
                if let Some(info) = &self.info {
                    info.panicked();
                }
                if let PanicPolicy::Shutdown(waker) = &self.panic_policy
                    && result_tx.shutdown().begin(ShutdownCause::panic(panic.as_ref())) == ShutdownStart::Started
                {
                    waker.wake_by_ref();
                }
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
        #[cfg(feature = "monitor")]
        if let Some(info) = &self.info {
            info.canceled();
        }
        discard_panicking_drop(body);
        drop(result_tx);
    }
}
