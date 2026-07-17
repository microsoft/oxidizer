// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deserialization into existing growable arena buffers.

use core::fmt;

use allocator_api2::alloc::Allocator;
use serde::de::{Deserializer, Error as _, SeqAccess, Visitor};

use super::{DeserializeIn, DeserializeInSeed};
use crate::strings::String;
use crate::vec::Vec;

mod deserialize_reuse;

use deserialize_reuse::DeserializeReuse;

impl<A: Allocator + Clone> String<'_, A> {
    /// Replaces this string from a deserializer while retaining reusable
    /// capacity.
    ///
    /// The string is cleared before reading input. If deserialization fails,
    /// it remains valid but may contain a partially deserialized value.
    ///
    /// ```
    /// use multitude::Arena;
    /// use serde::de::value::{Error, StrDeserializer};
    ///
    /// # fn main() -> Result<(), Error> {
    /// let arena = Arena::new();
    /// let mut text = arena.alloc_string_with_capacity(32);
    /// text.deserialize_reusing(StrDeserializer::new("replacement"))?;
    /// assert_eq!(text.as_str(), "replacement");
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error from the deserializer for invalid input or allocation
    /// failure.
    pub fn deserialize_reusing<'de, D>(&mut self, deserializer: D) -> Result<(), D::Error>
    where
        D: Deserializer<'de>,
    {
        <Self as DeserializeReuse<'de>>::deserialize_reusing(self, deserializer)
    }
}

impl<T, A: Allocator + Clone> Vec<'_, T, A> {
    /// Replaces this vector from a deserializer while retaining reusable
    /// capacity.
    ///
    /// The vector is cleared before reading input. If deserialization fails,
    /// it remains valid but may contain a partially deserialized prefix.
    ///
    /// ```
    /// use multitude::Arena;
    /// use serde::de::value::{Error, SeqDeserializer};
    ///
    /// # fn main() -> Result<(), Error> {
    /// let arena = Arena::new();
    /// let mut values: multitude::vec::Vec<'_, u64> = arena.alloc_vec_with_capacity(4);
    /// values.deserialize_reusing(SeqDeserializer::new([1_u64, 2, 3].into_iter()))?;
    /// assert_eq!(values.as_slice(), &[1_u64, 2, 3]);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error from the deserializer for invalid input or allocation
    /// failure.
    pub fn deserialize_reusing<'de, D>(&mut self, deserializer: D) -> Result<(), D::Error>
    where
        T: DeserializeIn<'de, A>,
        D: Deserializer<'de>,
    {
        <Self as DeserializeReuse<'de>>::deserialize_reusing(self, deserializer)
    }
}

impl<'de, A: Allocator + Clone> DeserializeReuse<'de> for String<'_, A> {
    fn deserialize_reusing<D>(&mut self, deserializer: D) -> Result<(), D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StringVisitor<'a, 'arena, A: Allocator + Clone>(&'a mut String<'arena, A>);

        impl<'de, A: Allocator + Clone> Visitor<'de> for StringVisitor<'_, '_, A> {
            type Value = ();

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                self.0.try_push_str(v).map_err(E::custom)
            }

            fn visit_borrowed_str<E: serde::de::Error>(self, v: &'de str) -> Result<Self::Value, E> {
                self.visit_str(v)
            }

            fn visit_string<E: serde::de::Error>(self, v: alloc::string::String) -> Result<Self::Value, E> {
                self.visit_str(&v)
            }
        }

        self.clear();
        deserializer.deserialize_str(StringVisitor(self))
    }
}

impl<'de, T, A> DeserializeReuse<'de> for Vec<'_, T, A>
where
    T: DeserializeIn<'de, A>,
    A: Allocator + Clone,
{
    fn deserialize_reusing<D>(&mut self, deserializer: D) -> Result<(), D::Error>
    where
        D: Deserializer<'de>,
    {
        struct VecVisitor<'a, 'arena, T, A: Allocator + Clone> {
            values: &'a mut Vec<'arena, T, A>,
            arena: &'arena crate::Arena<A>,
        }

        impl<'de, T, A> Visitor<'de> for VecVisitor<'_, '_, T, A>
        where
            T: DeserializeIn<'de, A>,
            A: Allocator + Clone,
        {
            type Value = ();

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence")
            }

            fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
            where
                S: SeqAccess<'de>,
            {
                if let Some(additional) = seq.size_hint() {
                    self.values.try_reserve(additional).map_err(S::Error::custom)?;
                }
                while let Some(value) = seq.next_element_seed(DeserializeInSeed::<T, A>::new(self.arena))? {
                    self.values.try_push(value).map_err(S::Error::custom)?;
                }
                Ok(())
            }
        }

        let arena = self.arena();
        self.clear();
        deserializer.deserialize_seq(VecVisitor { values: self, arena })
    }
}
