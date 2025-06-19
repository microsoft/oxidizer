// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::pal::{
    CompletionNotificationFacade, CompletionQueue, CompletionQueueWakerFacade,
    CompletionQueueWakerImpl, PrimitiveFacade,
};

#[derive(Debug, Default)]
pub struct CompletionQueueImpl;

impl CompletionQueue for CompletionQueueImpl {
    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn bind(&self, _primitive: &PrimitiveFacade) -> crate::Result<()> {
        todo!()
    }

    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn process_completions<CB>(&mut self, _max_wait_time_millis: u32, _cb: CB)
    where
        CB: FnMut(&CompletionNotificationFacade),
    {
        // TODO: this code intentionally does not throws so we can use the IO driver with its minimal
        // form even on Linux. This requires proper implementation further down the line.
    }

    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn waker(&self) -> CompletionQueueWakerFacade {
        CompletionQueueWakerFacade::Real(CompletionQueueWakerImpl)
    }
}