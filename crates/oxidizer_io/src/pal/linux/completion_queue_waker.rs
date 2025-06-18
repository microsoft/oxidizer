// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::pal::CompletionQueueWaker;

#[derive(Clone, Debug, Default)]
pub struct CompletionQueueWakerImpl;

impl CompletionQueueWaker for CompletionQueueWakerImpl {
    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn wake(&self) {
        // TODO: Do not panic, so calls to this method work. Requires proper implementation later.
    }
}