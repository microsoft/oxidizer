// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! JSON convenience APIs.

use allocator_api2::alloc::Allocator;

use super::{DeserializationLimits, DeserializeIn};
use crate::{Alloc, Arena};

impl<A: Allocator + Clone> Arena<A> {
    /// Deserialize JSON into an arena-aware value.
    ///
    /// Strings represented by arena-aware fields are copied into the arena.
    /// [`crate::Cow<'de, str>`] fields borrow unescaped strings directly from
    /// `input` and copy strings that require decoding. The return type selects
    /// root ownership.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON, a shape mismatch, trailing input,
    /// or allocation failure.
    ///
    /// ```
    /// # #[cfg(feature = "serde_json")]
    /// # fn main() -> Result<(), serde_json::Error> {
    /// use multitude::{Arc, Arena, Box, Rc};
    ///
    /// let arena = Arena::new();
    /// let boxed: Box<u64> = arena.deserialize_json("21")?;
    /// let shared: Arc<u64> = arena.deserialize_json(b"22")?;
    /// let local: Rc<u64> = arena.deserialize_json("23")?;
    /// assert_eq!((*boxed, *shared, *local), (21, 22, 23));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "serde_json"))]
    /// # fn main() {}
    /// ```
    pub fn deserialize_json<'de, T, I>(&self, input: &'de I) -> serde_json::Result<T>
    where
        T: DeserializeIn<'de, A>,
        I: AsRef<[u8]> + ?Sized,
    {
        let mut deserializer = serde_json::Deserializer::from_slice(input.as_ref());
        let value = self.deserialize(&mut deserializer)?;
        deserializer.end()?;
        Ok(value)
    }

    /// Deserialize JSON and store its root in an arena-local [`Alloc`].
    ///
    /// Nested fields retain the storage forms declared by `T`.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON, a shape mismatch, trailing input,
    /// or allocation failure.
    pub fn deserialize_json_alloc<'arena, 'de, T, I>(&'arena self, input: &'de I) -> serde_json::Result<Alloc<'arena, T>>
    where
        T: DeserializeIn<'de, A>,
        I: AsRef<[u8]> + ?Sized,
    {
        let mut deserializer = serde_json::Deserializer::from_slice(input.as_ref());
        let value = self.deserialize_alloc(&mut deserializer)?;
        deserializer.end()?;
        Ok(value)
    }

    /// Deserialize JSON into an arena-aware value while enforcing resource limits.
    ///
    /// As with [`Arena::deserialize_json`], the return type selects root
    /// ownership and both strings and byte inputs are accepted.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON, a shape mismatch, trailing input,
    /// allocation failure, or a limit violation.
    ///
    /// ```
    /// # #[cfg(feature = "serde_json")]
    /// # fn main() -> Result<(), serde_json::Error> {
    /// use multitude::de::DeserializationLimits;
    /// use multitude::{Arena, Box};
    ///
    /// let arena = Arena::new();
    /// let value: Box<u64> =
    ///     arena.deserialize_json_with_limits("24", DeserializationLimits::unlimited())?;
    /// assert_eq!(*value, 24);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "serde_json"))]
    /// # fn main() {}
    /// ```
    pub fn deserialize_json_with_limits<'de, T, I>(&self, input: &'de I, limits: DeserializationLimits) -> serde_json::Result<T>
    where
        T: DeserializeIn<'de, A>,
        I: AsRef<[u8]> + ?Sized,
    {
        let mut deserializer = serde_json::Deserializer::from_slice(input.as_ref());
        let value = self.deserialize_with_limits(&mut deserializer, limits)?;
        deserializer.end()?;
        Ok(value)
    }

    /// Deserialize JSON with resource limits and store its root in an
    /// arena-local [`Alloc`].
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON, a shape mismatch, trailing input,
    /// allocation failure, or a limit violation.
    pub fn deserialize_json_alloc_with_limits<'arena, 'de, T, I>(
        &'arena self,
        input: &'de I,
        limits: DeserializationLimits,
    ) -> serde_json::Result<Alloc<'arena, T>>
    where
        T: DeserializeIn<'de, A>,
        I: AsRef<[u8]> + ?Sized,
    {
        let mut deserializer = serde_json::Deserializer::from_slice(input.as_ref());
        let value = self.deserialize_alloc_with_limits(&mut deserializer, limits)?;
        deserializer.end()?;
        Ok(value)
    }
}
