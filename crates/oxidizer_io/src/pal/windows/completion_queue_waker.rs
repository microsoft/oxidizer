// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use crate::pal::{Bindings, BindingsFacade, CompletionPort, CompletionQueueWaker};

#[derive(Clone, Debug)]
pub struct CompletionQueueWakerImpl {
    completion_port: Arc<CompletionPort>,

    bindings: BindingsFacade,
}

impl CompletionQueueWakerImpl {
    pub(crate) const fn new(
        completion_port: Arc<CompletionPort>,
        bindings: BindingsFacade,
    ) -> Self {
        Self {
            completion_port,
            bindings,
        }
    }
}

impl CompletionQueueWaker for CompletionQueueWakerImpl {
    fn wake(&self) {
        // We do not care about the result - the call is unlikely to fail, except perhaps if the
        // I/O driver is already closed and there have been 9999 queued wake-ups for some reason
        // so the OS refuses to accept more. Very theoretically, and nothing we can do about it.
        _ = self.bindings.post_empty_queued_completion_status(
            *self.completion_port.as_handle(),
            super::constants::WAKE_UP_COMPLETION_KEY,
        );
    }
}