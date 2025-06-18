// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(test)]
use crate::pal::MockCompletionNotification;
use crate::pal::{CompletionNotification, CompletionNotificationImpl, ElementaryOperationKey};

#[derive(Debug)]
#[cfg_attr(not(test), repr(transparent))]
pub enum CompletionNotificationFacade {
    Real(CompletionNotificationImpl),

    #[cfg(test)]
    Mock(MockCompletionNotification),
}

impl CompletionNotificationFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_real(real: CompletionNotificationImpl) -> Self {
        Self::Real(real)
    }

    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_mock(mock: MockCompletionNotification) -> Self {
        Self::Mock(mock)
    }
}

impl From<CompletionNotificationImpl> for CompletionNotificationFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(real: CompletionNotificationImpl) -> Self {
        Self::from_real(real)
    }
}

#[cfg(test)]
impl From<MockCompletionNotification> for CompletionNotificationFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(mock: MockCompletionNotification) -> Self {
        Self::from_mock(mock)
    }
}

impl CompletionNotification for CompletionNotificationFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn elementary_operation_key(&self) -> ElementaryOperationKey {
        match self {
            Self::Real(real) => real.elementary_operation_key(),
            #[cfg(test)]
            Self::Mock(mock) => mock.elementary_operation_key(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn result(&self) -> crate::Result<u32> {
        match self {
            Self::Real(real) => real.result(),
            #[cfg(test)]
            Self::Mock(mock) => mock.result(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn is_wake_up_signal(&self) -> bool {
        match self {
            Self::Real(real) => real.is_wake_up_signal(),
            #[cfg(test)]
            Self::Mock(mock) => mock.is_wake_up_signal(),
        }
    }
}