// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deserialization capacity helper and the inherent `deserialize_in` methods
//! on the concrete lexicons.

use core::hash::BuildHasher;

use serde::Deserializer;

use super::DeserializeIn;
use crate::LocalLexicon;

/// Turns an untrusted length hint into a safe preallocation count.
///
/// Serde length hints are attacker-controlled: a malicious `serialize_seq`
/// header could claim a huge length to force an enormous upfront allocation
/// (a denial-of-service vector). We therefore cap the preallocated *bytes*, not
/// the element count, so the bound holds regardless of `T`'s size.
pub(crate) fn cautious_capacity<T>(hint: Option<usize>) -> usize {
    // 1 MiB is large enough that honest workloads almost never hit the cap, yet
    // small enough to bound a single speculative allocation from a bad hint.
    const MAX_PREALLOC_BYTES: usize = 1024 * 1024;

    hint.unwrap_or(0).min(MAX_PREALLOC_BYTES / core::mem::size_of::<T>().max(1))
}

impl<S: BuildHasher> LocalLexicon<S> {
    /// Deserialize a value, interning its [`Sym`](crate::Sym) fields into this
    /// lexicon.
    ///
    /// This is the interner-aware counterpart to [`serde::Deserialize`]. The
    /// return type `T` must implement [`DeserializeIn`], which the derive macro
    /// provides.
    ///
    /// # Errors
    ///
    /// Returns an error from the deserializer when the input is invalid.
    ///
    /// ```
    /// use internity::LocalLexicon;
    /// use internity::de::DeserializeIn;
    ///
    /// #[derive(DeserializeIn)]
    /// struct Record {
    ///     name: internity::Sym,
    /// }
    ///
    /// let mut lexicon = LocalLexicon::new();
    /// let record: Record = lexicon
    ///     .deserialize_in(&mut serde_json::Deserializer::from_str(r#"{"name":"a"}"#))
    ///     .unwrap();
    /// assert_eq!(lexicon.resolve(record.name), "a");
    /// ```
    /// Deserialization is not transactional: strings interned before an error
    /// remain in this lexicon.
    ///
    /// # Panics
    ///
    /// Panics if interning exceeds the lexicon's documented capacity limits.
    pub fn deserialize_in<'de, T, D>(&mut self, deserializer: D) -> Result<T, D::Error>
    where
        T: DeserializeIn<'de, Self>,
        D: Deserializer<'de>,
    {
        T::deserialize_in(self, deserializer)
    }
}

#[cfg(feature = "std")]
impl<S: BuildHasher> crate::ThreadedLexicon<S> {
    /// Deserialize a value, interning its [`Sym`](crate::Sym) fields into this
    /// concurrent lexicon.
    ///
    /// # Errors
    ///
    /// Returns an error from the deserializer when the input is invalid.
    ///
    /// ```
    /// use internity::de::DeserializeIn;
    /// use internity::{Reader, ThreadedLexicon};
    ///
    /// #[derive(DeserializeIn)]
    /// struct Record {
    ///     name: internity::Sym,
    /// }
    ///
    /// let lexicon = ThreadedLexicon::new();
    /// let record: Record = lexicon
    ///     .deserialize_in(&mut serde_json::Deserializer::from_str(r#"{"name":"a"}"#))
    ///     .unwrap();
    /// assert_eq!(lexicon.clone().freeze().resolve(record.name), "a");
    /// ```
    /// Deserialization is not transactional: strings interned before an error
    /// remain in this lexicon.
    ///
    /// # Panics
    ///
    /// Panics if interning exceeds a shard's documented capacity limits.
    pub fn deserialize_in<'de, T, D>(&self, deserializer: D) -> Result<T, D::Error>
    where
        T: DeserializeIn<'de, Self>,
        D: Deserializer<'de>,
    {
        T::deserialize_in(&mut self.clone(), deserializer)
    }
}
