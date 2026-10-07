// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::type_repetition_in_bounds,
    reason = "Fixture intentionally exercises separate inline and where-clause bounds"
)]

/// A generic service used to verify generic wrapper generation.
#[fakeable::fakeable(fake_impl = fakes::FakeGenericService<T>)]
pub struct GenericService<T: Clone>
where
    T: Send,
{
    value: T,
}

#[fakeable::fakeable]
impl<T: Clone> GenericService<T>
where
    T: Send,
{
    /// Creates a real service.
    pub fn new(value: T) -> Self {
        Self { value }
    }

    /// Returns the stored value.
    pub const fn value(&self) -> &T {
        &self.value
    }
}

#[cfg(any(feature = "test-util", test))]
pub mod fakes {
    /// A generic fake service.
    pub struct FakeGenericService<T>(pub T);

    impl<T> FakeGenericService<T> {
        /// Returns the fake value.
        pub const fn value(&self) -> &T {
            &self.0
        }
    }
}
