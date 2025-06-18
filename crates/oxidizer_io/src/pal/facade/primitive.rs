// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Display;

#[cfg(test)]
use crate::pal::MockPrimitive;
use crate::pal::{Primitive, PrimitiveImpl};

#[derive(Clone, Debug)]
pub enum PrimitiveFacade {
    Real(PrimitiveImpl),

    #[cfg(test)]
    Mock(MockPrimitive),
}

impl PrimitiveFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_real(real: PrimitiveImpl) -> Self {
        Self::Real(real)
    }

    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn from_mock(mock: MockPrimitive) -> Self {
        Self::Mock(mock)
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) const fn as_real(&self) -> &PrimitiveImpl {
        match self {
            Self::Real(real) => real,
            #[cfg(test)]
            _ => panic!("attempted to convert facade into the wrong type"),
        }
    }

    #[cfg(test)]
    #[expect(
        clippy::match_wildcard_for_single_variants,
        reason = "Intentional wildcard match"
    )]
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub(crate) fn as_mock(&self) -> &MockPrimitive {
        match self {
            Self::Mock(mock) => mock,
            _ => panic!("attempted to convert facade into the wrong type"),
        }
    }
}

impl From<PrimitiveImpl> for PrimitiveFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(real: PrimitiveImpl) -> Self {
        Self::from_real(real)
    }
}

#[cfg(test)]
impl From<MockPrimitive> for PrimitiveFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn from(mock: MockPrimitive) -> Self {
        Self::from_mock(mock)
    }
}

impl Primitive for PrimitiveFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn close(&self) {
        match self {
            Self::Real(real) => real.close(),
            #[cfg(test)]
            Self::Mock(mock) => mock.close(),
        }
    }
}

impl Display for PrimitiveFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Real(real) => Display::fmt(real, f),
            #[cfg(test)]
            Self::Mock(mock) => Display::fmt(mock, f),
        }
    }
}