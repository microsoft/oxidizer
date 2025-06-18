// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use crate::pal::MockPlatform;
use crate::pal::{
    BUILD_TARGET_PLATFORM, BuildTargetPlatform, CompletionQueueFacade, ElementaryOperationFacade,
    MemoryPoolFacade, Platform,
};

#[derive(Clone, Debug)]
pub enum PlatformFacade {
    Real(&'static BuildTargetPlatform),

    #[cfg(test)]
    Mock(Arc<MockPlatform>),
}

impl PlatformFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) fn real() -> Self {
        Self::Real(&BUILD_TARGET_PLATFORM)
    }

    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) fn from_mock(mock: MockPlatform) -> Self {
        Self::Mock(Arc::new(mock))
    }
}

#[cfg(test)]
impl From<MockPlatform> for PlatformFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(mock: MockPlatform) -> Self {
        Self::from_mock(mock)
    }
}

impl Platform for PlatformFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn new_completion_queue(&self) -> CompletionQueueFacade {
        match self {
            Self::Real(platform) => platform.new_completion_queue(),
            #[cfg(test)]
            Self::Mock(mock) => mock.new_completion_queue(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn new_elementary_operation(&self, offset: u64) -> ElementaryOperationFacade {
        match self {
            Self::Real(platform) => platform.new_elementary_operation(offset),
            #[cfg(test)]
            Self::Mock(mock) => mock.new_elementary_operation(offset),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn new_memory_pool(&self) -> MemoryPoolFacade {
        match self {
            Self::Real(platform) => platform.new_memory_pool(),
            #[cfg(test)]
            Self::Mock(mock) => mock.new_memory_pool(),
        }
    }
}