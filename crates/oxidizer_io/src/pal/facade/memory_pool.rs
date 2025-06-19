// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

use crate::mem::SequenceBuilder;
#[cfg(test)]
use crate::pal::MockMemoryPool;
use crate::pal::{DefaultMemoryPool, MemoryPool};

#[derive(Debug)]
pub enum MemoryPoolFacade {
    Real(DefaultMemoryPool),

    #[cfg(test)]
    Mock(MockMemoryPool),
}

impl MemoryPoolFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_real(real: DefaultMemoryPool) -> Self {
        Self::Real(real)
    }

    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_mock(mock: MockMemoryPool) -> Self {
        Self::Mock(mock)
    }
}

impl From<DefaultMemoryPool> for MemoryPoolFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(real: DefaultMemoryPool) -> Self {
        Self::from_real(real)
    }
}

#[cfg(test)]
impl From<MockMemoryPool> for MemoryPoolFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(mock: MockMemoryPool) -> Self {
        Self::from_mock(mock)
    }
}

impl MemoryPool for MemoryPoolFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn rent(&self, count_bytes: usize, preferred_block_size: NonZeroUsize) -> SequenceBuilder {
        match self {
            Self::Real(real) => real.rent(count_bytes, preferred_block_size),
            #[cfg(test)]
            Self::Mock(mock) => mock.rent(count_bytes, preferred_block_size),
        }
    }
}