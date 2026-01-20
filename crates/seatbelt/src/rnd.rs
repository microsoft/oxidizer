// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

/// Non-cryptographic random number generator used in this crate.
///
/// This random generator is **NOT cryptography secure** and should only be used for
/// non-security-critical purposes such as load balancing, jitter, sampling,
/// and other scenarios where cryptography guarantees are not required.
///
/// The `seatbelt` crate does not require cryptography security for its
/// random number generation needs, so this type is provided as a lightweight
/// alternative to more complex `RNG` implementations.
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
    pub fn new_fixed(value: f64) -> Self {
        Self::Test(std::sync::Arc::new(move || value))
    }

    #[cfg(test)]
    pub fn new_function<F>(f: F) -> Self
    where
        F: Fn() -> f64 + Send + Sync + 'static,
    {
        Self::Test(std::sync::Arc::new(f))
    }

    #[cfg_attr(test, mutants::skip)] // Mutating return value causes infinite loops in backoff calculations
    pub fn next_f64(&self) -> f64 {
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
