// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty::runtime::{Runtime, RuntimeOperations};
use arty::task::{JoinError, JoinHandle};

pub(crate) fn runtime_operations(runtime: &Runtime) -> RuntimeOperations {
    runtime
        .scheduler()
        .block_on(async |builtins| builtins.operations().clone())
        .expect("runtime remains active while acquiring test operations")
}

const _: fn(&Runtime) -> RuntimeOperations = runtime_operations;

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
