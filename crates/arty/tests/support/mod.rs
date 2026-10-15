// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty::task::{JoinError, JoinHandle};

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
