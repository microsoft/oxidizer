// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Trait used to verify fully qualified delegation.
pub trait Value {
    /// Returns the trait-specific value.
    fn value(&self) -> i32;

    /// Duplicates the service.
    #[must_use]
    fn duplicate(&self) -> Self;
}

/// A service with an inherent method that intentionally collides with its trait method.
#[fakeable::fakeable(fake_impl = fakes::FakeTraitService)]
pub struct TraitService;

#[fakeable::fakeable]
impl TraitService {
    /// Creates a real service.
    pub const fn new() -> Self {
        Self
    }

    /// Returns the inherent value.
    pub const fn value(&self) -> i32 {
        1
    }
}

#[fakeable::fakeable]
impl Value for TraitService {
    fn value(&self) -> i32 {
        2
    }

    fn duplicate(&self) -> Self {
        Self
    }
}

#[cfg(any(feature = "test-util", test))]
pub mod fakes {
    /// Fake implementation for [`super::TraitService`].
    pub struct FakeTraitService;

    impl FakeTraitService {
        /// Returns the fake trait value.
        #[must_use]
        pub const fn value(&self) -> i32 {
            3
        }

        /// Duplicates the fake service.
        #[must_use]
        pub const fn duplicate(&self) -> Self {
            Self
        }
    }
}
