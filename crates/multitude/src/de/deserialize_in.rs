// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::Allocator;
use serde::de::Deserializer;

use crate::Arena;

/// Deserialize a value using storage supplied by an [`Arena`].
///
/// Unlike [`serde::Deserialize`], this trait receives the arena instance that
/// must back allocator-aware fields. Implementations should use
/// [`super::DeserializeInSeed`] when recursively deserializing nested values.
/// Most applications should derive this trait and call [`Arena::deserialize`]
/// rather than invoke [`DeserializeIn::deserialize_in`] directly.
///
/// ```
/// use multitude::Arena;
/// use multitude::de::DeserializeIn;
/// use serde::de::value::{Error, U64Deserializer};
///
/// # fn main() -> Result<(), Error> {
/// let arena = Arena::new();
/// let value = u64::deserialize_in(&arena, U64Deserializer::new(42))?;
/// assert_eq!(value, 42);
/// # Ok(())
/// # }
/// ```
pub trait DeserializeIn<'de, A: Allocator + Clone>: Sized {
    /// Deserialize `Self`, allocating owned storage from `arena`.
    ///
    /// # Errors
    ///
    /// Returns an error from the deserializer when the input is invalid or the
    /// arena cannot satisfy an allocation. Allocation errors are reported
    /// through [`serde::de::Error::custom`].
    fn deserialize_in<D>(arena: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>;
}
