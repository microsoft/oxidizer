// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! JSON convenience APIs.

use core::fmt;

use allocator_api2::alloc::Allocator;
use serde::de::{DeserializeSeed, Error as _, SeqAccess, Visitor};

use super::{DeserializationLimits, DeserializeIn, DeserializeInSeed, JsonError};
use crate::vec::Vec;
use crate::{Alloc, Arena};

struct DeserializeEachSeed<'arena, 'callback, T, A: Allocator + Clone, F> {
    arena: &'arena Arena<A>,
    callback: &'callback mut F,
    marker: core::marker::PhantomData<fn() -> T>,
}

impl<'de, T, A, F> DeserializeSeed<'de> for DeserializeEachSeed<'_, '_, T, A, F>
where
    T: DeserializeIn<'de, A>,
    A: Allocator + Clone,
    F: FnMut(T),
{
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct EachVisitor<'arena, 'callback, T, A: Allocator + Clone, F> {
            arena: &'arena Arena<A>,
            callback: &'callback mut F,
            marker: core::marker::PhantomData<fn() -> T>,
        }

        impl<'de, T, A, F> Visitor<'de> for EachVisitor<'_, '_, T, A, F>
        where
            T: DeserializeIn<'de, A>,
            A: Allocator + Clone,
            F: FnMut(T),
        {
            type Value = ();

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence")
            }

            fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
            where
                S: SeqAccess<'de>,
            {
                while let Some(item) = seq.next_element_seed(DeserializeInSeed::<T, A>::new(self.arena))? {
                    (self.callback)(item);
                }
                Ok(())
            }
        }

        deserializer.deserialize_seq(EachVisitor {
            arena: self.arena,
            callback: self.callback,
            marker: core::marker::PhantomData,
        })
    }
}

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
    /// Returns [`JsonError`] for malformed JSON, a shape mismatch, trailing
    /// input, allocation failure, or a limit violation. Use
    /// [`JsonError::limit_exceeded`] to identify resource rejection.
    ///
    /// ```
    /// # #[cfg(feature = "serde_json")]
    /// # fn main() -> Result<(), multitude::de::JsonError> {
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
    pub fn deserialize_json_with_limits<'de, T, I>(&self, input: &'de I, limits: DeserializationLimits) -> Result<T, JsonError>
    where
        T: DeserializeIn<'de, A>,
        I: AsRef<[u8]> + ?Sized,
    {
        let mut deserializer = serde_json::Deserializer::from_slice(input.as_ref());
        let (value, limit_exceeded) =
            super::limits::deserialize_seed_with_limits_detailed(&mut deserializer, DeserializeInSeed::<T, A>::new(self), limits);
        let value = value.map_err(|source| JsonError::new(source, limit_exceeded))?;
        deserializer.end().map_err(JsonError::from)?;
        Ok(value)
    }

    /// Deserialize JSON with resource limits and store its root in an
    /// arena-local [`Alloc`].
    ///
    /// # Errors
    ///
    /// Returns [`JsonError`] for malformed JSON, a shape mismatch, trailing
    /// input, allocation failure, or a limit violation. Use
    /// [`JsonError::limit_exceeded`] to identify resource rejection.
    pub fn deserialize_json_alloc_with_limits<'arena, 'de, T, I>(
        &'arena self,
        input: &'de I,
        limits: DeserializationLimits,
    ) -> Result<Alloc<'arena, T>, JsonError>
    where
        T: DeserializeIn<'de, A>,
        I: AsRef<[u8]> + ?Sized,
    {
        let mut deserializer = serde_json::Deserializer::from_slice(input.as_ref());
        let (value, limit_exceeded) =
            super::limits::deserialize_seed_with_limits_detailed(&mut deserializer, DeserializeInSeed::<T, A>::new(self), limits);
        let value = value.map_err(|source| JsonError::new(source, limit_exceeded))?;
        let value = self
            .try_alloc(value)
            .map_err(|error| JsonError::from(serde_json::Error::custom(error)))?;
        deserializer.end().map_err(JsonError::from)?;
        Ok(value)
    }

    /// Deserialize each element of a complete top-level JSON array.
    ///
    /// Elements are passed to `callback` in input order without constructing a
    /// root collection. The callback receives ownership of each value and may
    /// move selected arena-owned fields into longer-lived storage.
    ///
    /// Use [`serde_json::value::RawValue`] as the element type to inspect
    /// syntactically valid elements before materializing selected records. Raw
    /// values borrow `input` and do not allocate.
    ///
    /// If deserialization fails, the callback may already have processed a
    /// prefix of the input. Trailing input is checked after every array element
    /// has been delivered.
    ///
    /// # Errors
    ///
    /// Returns an error if the root is not an array, or for malformed JSON, a
    /// shape mismatch, trailing input, or allocation failure.
    ///
    /// ```
    /// # #[cfg(feature = "serde_json")]
    /// # fn main() -> Result<(), serde_json::Error> {
    /// let arena = multitude::Arena::new();
    /// let mut sum = 0;
    /// arena.deserialize_json_each("[1,2,3]", |value: u64| sum += value)?;
    /// assert_eq!(sum, 6);
    ///
    /// let mut raw = Vec::new();
    /// arena.deserialize_json_each(
    ///     "[{\"id\":1},{\"id\":2}]",
    ///     |value: &serde_json::value::RawValue| {
    ///         raw.push(value.get());
    ///     },
    /// )?;
    /// assert_eq!(raw, [r#"{"id":1}"#, r#"{"id":2}"#]);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "serde_json"))]
    /// # fn main() {}
    /// ```
    pub fn deserialize_json_each<'de, T, I, F>(&self, input: &'de I, mut callback: F) -> serde_json::Result<()>
    where
        T: DeserializeIn<'de, A>,
        I: AsRef<[u8]> + ?Sized,
        F: FnMut(T),
    {
        let mut deserializer = serde_json::Deserializer::from_slice(input.as_ref());
        DeserializeEachSeed {
            arena: self,
            callback: &mut callback,
            marker: core::marker::PhantomData,
        }
        .deserialize(&mut deserializer)?;
        deserializer.end()
    }

    /// Deserialize each element of a complete top-level JSON array while
    /// enforcing resource limits.
    ///
    /// This has the same prefix-on-failure and complete-input behavior as
    /// [`Self::deserialize_json_each`]. The top-level array counts as a
    /// sequence for [`DeserializationLimits::max_sequence_len`].
    /// [`serde_json::value::RawValue`] elements are opaque to nested limits;
    /// deserialize selected raw values with a limited API when their contents
    /// also need enforcement.
    ///
    /// # Errors
    ///
    /// Returns [`JsonError`] if the root is not an array, or for malformed
    /// JSON, a shape mismatch, trailing input, allocation failure, or a limit
    /// violation. Use [`JsonError::limit_exceeded`] to identify resource
    /// rejection.
    pub fn deserialize_json_each_with_limits<'de, T, I, F>(
        &self,
        input: &'de I,
        limits: DeserializationLimits,
        mut callback: F,
    ) -> Result<(), JsonError>
    where
        T: DeserializeIn<'de, A>,
        I: AsRef<[u8]> + ?Sized,
        F: FnMut(T),
    {
        let mut deserializer = serde_json::Deserializer::from_slice(input.as_ref());
        let (result, limit_exceeded) = super::limits::deserialize_seed_with_limits_detailed(
            &mut deserializer,
            DeserializeEachSeed {
                arena: self,
                callback: &mut callback,
                marker: core::marker::PhantomData,
            },
            limits,
        );
        result.map_err(|source| JsonError::new(source, limit_exceeded))?;
        deserializer.end().map_err(JsonError::from)
    }
}

impl<T, A: Allocator + Clone> Vec<'_, T, A> {
    /// Replaces this vector from complete JSON input while retaining its
    /// capacity.
    ///
    /// Reuse is useful for several refreshes between arena resets. The vector
    /// borrows its arena, so it must be dropped before [`Arena::reset`] and
    /// recreated afterward from the arena's warm chunk cache.
    ///
    /// If deserialization fails, the vector remains valid but may contain a
    /// partially deserialized prefix.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed JSON, a shape mismatch, trailing input,
    /// or allocation failure.
    ///
    /// ```
    /// # #[cfg(feature = "serde_json")]
    /// # fn main() -> Result<(), serde_json::Error> {
    /// let arena = multitude::Arena::new();
    /// let mut values = arena.alloc_vec_with_capacity::<u64>(4);
    /// values.deserialize_json_reusing("[1,2,3]")?;
    /// assert_eq!(values.as_slice(), &[1, 2, 3]);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "serde_json"))]
    /// # fn main() {}
    /// ```
    pub fn deserialize_json_reusing<'de, I>(&mut self, input: &'de I) -> serde_json::Result<()>
    where
        T: DeserializeIn<'de, A>,
        I: AsRef<[u8]> + ?Sized,
    {
        let mut deserializer = serde_json::Deserializer::from_slice(input.as_ref());
        self.deserialize_reusing(&mut deserializer)?;
        deserializer.end()
    }

    /// Replaces this vector from complete JSON input while retaining its
    /// capacity and enforcing resource limits.
    ///
    /// If deserialization fails, the vector remains valid but may contain a
    /// partially deserialized prefix.
    ///
    /// # Errors
    ///
    /// Returns [`JsonError`] for malformed JSON, a shape mismatch, trailing
    /// input, allocation failure, or a limit violation. Use
    /// [`JsonError::limit_exceeded`] to identify resource rejection.
    pub fn deserialize_json_reusing_with_limits<'de, I>(&mut self, input: &'de I, limits: DeserializationLimits) -> Result<(), JsonError>
    where
        T: DeserializeIn<'de, A>,
        I: AsRef<[u8]> + ?Sized,
    {
        let mut deserializer = serde_json::Deserializer::from_slice(input.as_ref());
        let (result, limit_exceeded) = self.deserialize_reusing_with_limits_detailed(&mut deserializer, limits);
        result.map_err(|source| JsonError::new(source, limit_exceeded))?;
        deserializer.end().map_err(JsonError::from)
    }
}
