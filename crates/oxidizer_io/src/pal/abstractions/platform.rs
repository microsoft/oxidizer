// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::pal::{CompletionQueueFacade, ElementaryOperationFacade, MemoryPoolFacade};

pub trait Platform: Debug + Send + Sync + 'static {
    fn new_completion_queue(&self) -> CompletionQueueFacade;

    fn new_elementary_operation(&self, offset: u64) -> ElementaryOperationFacade;

    fn new_memory_pool(&self) -> MemoryPoolFacade;
}