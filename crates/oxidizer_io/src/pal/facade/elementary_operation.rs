// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;

#[cfg(test)]
use crate::pal::MockElementaryOperation;
use crate::pal::{ElementaryOperation, ElementaryOperationImpl, ElementaryOperationKey};

#[derive(Debug)]
pub enum ElementaryOperationFacade {
    Real(ElementaryOperationImpl),

    #[cfg(test)]
    Mock(MockElementaryOperation),
}

impl ElementaryOperationFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_real(real: ElementaryOperationImpl) -> Self {
        Self::Real(real)
    }

    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_mock(mock: MockElementaryOperation) -> Self {
        Self::Mock(mock)
    }
}

impl From<ElementaryOperationImpl> for ElementaryOperationFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(real: ElementaryOperationImpl) -> Self {
        Self::from_real(real)
    }
}

#[cfg(test)]
impl From<MockElementaryOperation> for ElementaryOperationFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(mock: MockElementaryOperation) -> Self {
        Self::from_mock(mock)
    }
}

impl ElementaryOperation for ElementaryOperationFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn key(self: Pin<&Self>) -> ElementaryOperationKey {
        let this = self.get_ref();

        match this {
            Self::Real(real) => {
                // SAFETY: We already have pinned self, the pin marker just got lost along the way.
                let real_as_pinned = unsafe { Pin::new_unchecked(real) };
                real_as_pinned.key()
            }
            #[cfg(test)]
            Self::Mock(mock) => {
                // SAFETY: We already have pinned self, the pin marker just got lost along the way.
                let mock_as_pinned = unsafe { Pin::new_unchecked(mock) };
                mock_as_pinned.key()
            }
        }
    }
}