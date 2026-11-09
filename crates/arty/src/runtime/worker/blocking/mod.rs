// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Blocking worker admission and pool execution.
//!
//! `worker` accepts and runs blocking callbacks. `pools` owns the blocking
//! thread pools. `local_worker` stores thread-local worker state.

mod local_worker;
mod pools;
mod worker;

pub(crate) use local_worker::LocalWorker;
pub(crate) use pools::Pools;
pub(crate) use worker::{Pool, Worker};

pub(crate) type PoolIdentity = performables::arc::Arc<()>;

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) enum TestPanicResponse {
    Ignore,
    Shutdown(std::task::Waker),
}

#[cfg(test)]
impl From<TestPanicResponse> for crate::runtime::hook::BlockingPanicHook {
    fn from(value: TestPanicResponse) -> Self {
        match value {
            TestPanicResponse::Ignore => Self::for_test(crate::runtime::hook::PanicAction::Ignore),
            TestPanicResponse::Shutdown(waker) => Self::for_test_with_waker(crate::runtime::hook::PanicAction::Shutdown, waker),
        }
    }
}
