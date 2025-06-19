// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::pal::{CompletionQueue, CompletionQueueImpl, PrimitiveFacade};
#[cfg(test)]
use crate::testing::SimulatedCompletionQueue;

#[derive(Debug)]
pub enum CompletionQueueFacade {
    Real(CompletionQueueImpl),

    #[cfg(test)]
    Simulated(SimulatedCompletionQueue),
}

impl CompletionQueueFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_real(real: CompletionQueueImpl) -> Self {
        Self::Real(real)
    }

    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_simulated(simulated: SimulatedCompletionQueue) -> Self {
        Self::Simulated(simulated)
    }
}

impl From<CompletionQueueImpl> for CompletionQueueFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(real: CompletionQueueImpl) -> Self {
        Self::from_real(real)
    }
}

#[cfg(test)]
impl From<SimulatedCompletionQueue> for CompletionQueueFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(simulated: SimulatedCompletionQueue) -> Self {
        Self::from_simulated(simulated)
    }
}

impl CompletionQueue for CompletionQueueFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn bind(&self, primitive: &PrimitiveFacade) -> crate::Result<()> {
        match self {
            Self::Real(real) => real.bind(primitive),
            #[cfg(test)]
            Self::Simulated(simulated) => simulated.bind(primitive),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn process_completions<CB>(&mut self, max_wait_time_millis: u32, cb: CB)
    where
        CB: FnMut(&super::CompletionNotificationFacade),
    {
        match self {
            Self::Real(real) => real.process_completions(max_wait_time_millis, cb),
            #[cfg(test)]
            Self::Simulated(simulated) => {
                simulated.process_completions(max_wait_time_millis, cb);
            }
        }
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn waker(&self) -> super::CompletionQueueWakerFacade {
        match self {
            Self::Real(real) => real.waker(),
            #[cfg(test)]
            Self::Simulated(simulated) => simulated.waker(),
        }
    }
}