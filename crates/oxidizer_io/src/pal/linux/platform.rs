// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

use crate::pal::{
    CompletionQueueFacade, CompletionQueueImpl, DefaultMemoryPool, ElementaryOperationFacade,
    MemoryPoolFacade, Platform,
};

#[derive(Debug)]
pub struct BuildTargetPlatform;

impl Platform for BuildTargetPlatform {
    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn new_completion_queue(&self) -> CompletionQueueFacade {
        CompletionQueueFacade::Real(CompletionQueueImpl)
    }

    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn new_elementary_operation(&self, _offset: u64) -> ElementaryOperationFacade {
        todo!()
    }

    #[cfg_attr(test, mutants::skip)] // Linux is just a placeholder to validate "builds, not runs".
    fn new_memory_pool(&self) -> MemoryPoolFacade {
        // We pick an unusual block size for now, to help ensure that implementations do not take a
        // dependency on some specific "round number" block size.
        const BLOCK_SIZE: NonZeroUsize = NonZeroUsize::new(2137).unwrap();

        DefaultMemoryPool::new(BLOCK_SIZE).into()
    }
}