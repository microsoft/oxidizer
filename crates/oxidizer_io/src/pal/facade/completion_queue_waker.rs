// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::pal::{CompletionQueueWaker, CompletionQueueWakerImpl};
#[cfg(test)]
use crate::testing::SimulatedCompletionQueueWaker;

#[derive(Clone, Debug)]
pub enum CompletionQueueWakerFacade {
    Real(CompletionQueueWakerImpl),

    #[cfg(test)]
    Simulated(SimulatedCompletionQueueWaker),
}

impl CompletionQueueWakerFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_real(real: CompletionQueueWakerImpl) -> Self {
        Self::Real(real)
    }

    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_simulated(simulated: SimulatedCompletionQueueWaker) -> Self {
        Self::Simulated(simulated)
    }
}

impl From<CompletionQueueWakerImpl> for CompletionQueueWakerFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(real: CompletionQueueWakerImpl) -> Self {
        Self::from_real(real)
    }
}

#[cfg(test)]
impl From<SimulatedCompletionQueueWaker> for CompletionQueueWakerFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(simulated: SimulatedCompletionQueueWaker) -> Self {
        Self::from_simulated(simulated)
    }
}

impl CompletionQueueWaker for CompletionQueueWakerFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn wake(&self) {
        match self {
            Self::Real(real) => real.wake(),
            #[cfg(test)]
            Self::Simulated(simulated) => simulated.wake(),
        }
    }
}