// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;

use serde::de::Deserializer;

/// A seed used by the derive macro for fields explicitly delegated to Serde.
///
/// ```
/// use multitude::de::DeserializeSeed;
/// use serde::de::DeserializeSeed as _;
/// use serde::de::value::{Error, U64Deserializer};
///
/// let value = DeserializeSeed::<u32>::new().deserialize(U64Deserializer::<Error>::new(7))?;
/// assert_eq!(value, 7);
/// # Ok::<(), Error>(())
/// ```
#[doc(hidden)]
#[derive(Debug)]
pub struct DeserializeSeed<T>(PhantomData<fn() -> T>);

impl<T> DeserializeSeed<T> {
    /// Create a stateless Serde seed.
    ///
    /// ```
    /// use multitude::de::DeserializeSeed;
    ///
    /// let _: DeserializeSeed<u32> = DeserializeSeed::new();
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T> Default for DeserializeSeed<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'de, T: serde::Deserialize<'de>> serde::de::DeserializeSeed<'de> for DeserializeSeed<T> {
    type Value = T;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer)
    }
}
