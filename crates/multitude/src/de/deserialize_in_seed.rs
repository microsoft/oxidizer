// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;

use allocator_api2::alloc::Allocator;
use serde::de::Deserializer;

use super::DeserializeIn;
use crate::Arena;

/// A Serde seed that propagates an arena to a [`DeserializeIn`] value.
///
/// ```
/// use multitude::Arena;
/// use multitude::de::DeserializeInSeed;
/// use serde::de::DeserializeSeed;
/// use serde::de::value::{Error, U64Deserializer};
///
/// # fn main() -> Result<(), Error> {
/// let arena = Arena::new();
/// let seed = DeserializeInSeed::<u64, _>::new(&arena);
/// assert_eq!(seed.deserialize(U64Deserializer::new(7))?, 7);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct DeserializeInSeed<'a, T, A: Allocator + Clone> {
    arena: &'a Arena<A>,
    marker: PhantomData<fn() -> T>,
}

impl<'a, T, A: Allocator + Clone> DeserializeInSeed<'a, T, A> {
    /// Create a seed backed by `arena`.
    ///
    /// ```
    /// use multitude::Arena;
    /// use multitude::de::DeserializeInSeed;
    /// use serde::de::DeserializeSeed;
    /// use serde::de::value::{Error, U64Deserializer};
    ///
    /// # fn main() -> Result<(), Error> {
    /// let arena = Arena::new();
    /// let seed = DeserializeInSeed::<u64, _>::new(&arena);
    /// assert_eq!(seed.deserialize(U64Deserializer::new(9))?, 9);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn new(arena: &'a Arena<A>) -> Self {
        Self {
            arena,
            marker: PhantomData,
        }
    }
}

impl<'de, T, A> serde::de::DeserializeSeed<'de> for DeserializeInSeed<'_, T, A>
where
    T: DeserializeIn<'de, A>,
    A: Allocator + Clone,
{
    type Value = T;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize_in(self.arena, deserializer)
    }
}
