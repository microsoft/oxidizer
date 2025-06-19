// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

use crate::pal::{
    BindingsFacade, CompletionQueueFacade, CompletionQueueImpl, DefaultMemoryPool,
    ElementaryOperationFacade, ElementaryOperationImpl, MemoryPoolFacade, Platform,
};

/// The platform that matches the crate's build target.
///
/// You would only use a different platform in unit tests that need to mock the platform.
/// Even then, whenever possible, unit tests should use the real platform for maximum realism.
#[derive(Debug)]
pub struct BuildTargetPlatform {
    bindings: BindingsFacade,
}

impl BuildTargetPlatform {
    pub(crate) const fn new(bindings: BindingsFacade) -> Self {
        Self { bindings }
    }
}

impl Platform for BuildTargetPlatform {
    fn new_completion_queue(&self) -> CompletionQueueFacade {
        CompletionQueueImpl::new(self.bindings.clone()).into()
    }

    fn new_elementary_operation(&self, offset: u64) -> ElementaryOperationFacade {
        ElementaryOperationImpl::new(offset).into()
    }

    fn new_memory_pool(&self) -> MemoryPoolFacade {
        // We pick an unusual block size for now, to help ensure that implementations do not take a
        // dependency on some specific "round number" block size.
        const BLOCK_SIZE: NonZeroUsize = NonZeroUsize::new(2137).unwrap();

        DefaultMemoryPool::new(BLOCK_SIZE).into()
    }
}