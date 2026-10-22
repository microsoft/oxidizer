// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty::runtime::{Runtime, RuntimeControl};
use arty::task::{JoinError, JoinHandle};

pub(crate) fn runtime_control(runtime: &Runtime) -> RuntimeControl {
    runtime
        .scheduler()
        .block_on(async |builtins| builtins.control().clone())
        .expect("runtime remains active while acquiring test control")
}

const _: fn(&Runtime) -> RuntimeControl = runtime_control;

pub(crate) trait JoinHandleExt {
    type Output;

    fn join(self) -> Result<Self::Output, JoinError>;
}

impl<R> JoinHandleExt for JoinHandle<R>
where
    R: Send + 'static,
{
    type Output = R;

    fn join(self) -> Result<Self::Output, JoinError> {
        futures::executor::block_on(self)
    }
}
