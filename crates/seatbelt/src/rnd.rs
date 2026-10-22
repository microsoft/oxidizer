// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

/// Generates random numbers for non-security uses in this crate.
///
/// Do not use this generator for cryptography.
/// Use it for load balancing, jitter, sampling, and other non-security tasks.
///
/// Seatbelt does not need cryptographically secure random numbers.
/// This type is a small alternative to more complex `RNG` implementations.
#[derive(Clone, Default)]
pub(crate) enum Rnd {
    #[default]
    Real,

    #[cfg(test)]
    Test(std::sync::Arc<dyn Fn() -> f64 + Send + Sync>),
}

impl Debug for Rnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Real => write!(f, "Real"),
            #[cfg(test)]
            Self::Test(_) => write!(f, "Test"),
        }
    }
}

impl Rnd {
    #[cfg(test)]
    pub(crate) fn new_fixed(value: f64) -> Self {
        Self::Test(std::sync::Arc::new(move || value))
    }

    #[cfg(test)]
    pub(crate) fn new_function<F>(f: F) -> Self
    where
        F: Fn() -> f64 + Send + Sync + 'static,
    {
        Self::Test(std::sync::Arc::new(f))
    }

    #[cfg_attr(test, mutants::skip)] // Mutating return value causes infinite loops in backoff calculations
    pub(crate) fn next_f64(&self) -> f64 {
        match self {
            Self::Real => fastrand::f64(),
            #[cfg(test)]
            Self::Test(generator) => generator(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_real() {
        assert_eq!(format!("{:?}", Rnd::Real), "Real");
    }

    #[test]
    fn debug_test() {
        assert_eq!(format!("{:?}", Rnd::new_fixed(0.5)), "Test");
    }
}
