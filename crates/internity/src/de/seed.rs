// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::marker::PhantomData;

use serde::de::Deserializer;

use super::DeserializeIn;
use crate::Lexicon;

/// A Serde seed that propagates a [`Lexicon`] to a [`DeserializeIn`] value.
///
/// Use this when integrating interner-aware values into another Serde
/// [`Visitor`](serde::de::Visitor); the derive macro emits it for each field.
///
/// ```
/// use internity::LocalLexicon;
/// use internity::de::DeserializeInSeed;
/// use serde::de::DeserializeSeed as _;
///
/// let mut lexicon = LocalLexicon::new();
/// let seed = DeserializeInSeed::<internity::Sym, _>::new(&mut lexicon);
/// let sym = seed
///     .deserialize(&mut serde_json::Deserializer::from_str("\"hello\""))
///     .unwrap();
/// assert_eq!(lexicon.resolve(sym), "hello");
/// ```
#[derive(Debug)]
pub struct DeserializeInSeed<'a, T, I: Lexicon + ?Sized> {
    interner: &'a mut I,
    marker: PhantomData<fn() -> T>,
}

impl<'a, T, I: Lexicon + ?Sized> DeserializeInSeed<'a, T, I> {
    /// Create a seed backed by `interner`.
    #[must_use]
    pub const fn new(interner: &'a mut I) -> Self {
        Self {
            interner,
            marker: PhantomData,
        }
    }
}

impl<'de, T, I> serde::de::DeserializeSeed<'de> for DeserializeInSeed<'_, T, I>
where
    T: DeserializeIn<'de, I>,
    I: Lexicon + ?Sized,
{
    type Value = T;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize_in(self.interner, deserializer)
    }
}

/// A stateless seed used by the derive macro for fields delegated to Serde via
/// `#[internity(via_serde)]`.
#[doc(hidden)]
#[derive(Debug)]
pub struct DeserializeSeed<T>(PhantomData<fn() -> T>);

impl<T> DeserializeSeed<T> {
    /// Create a stateless Serde seed.
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
