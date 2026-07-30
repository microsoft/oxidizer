// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use serde::Deserializer;

use crate::Lexicon;

/// Deserialize a value while interning its [`Sym`](crate::Sym) fields.
///
/// Unlike [`serde::Deserialize`], this trait receives the [`Lexicon`] that
/// backs [`Sym`](crate::Sym) fields. Implementations use
/// [`DeserializeInSeed`](super::DeserializeInSeed) when recursively
/// deserializing nested values.
/// Most applications derive this trait and call [`LocalLexicon::deserialize_in`] or
/// [`ThreadedLexicon::deserialize_in`] rather than invoke [`deserialize_in`]
/// directly.
///
/// The trait is generic over the interner `I`, so a derived implementation
/// works with any [`Lexicon`], including both [`LocalLexicon`] and
/// [`ThreadedLexicon`].
///
/// [`Lexicon`]: crate::Lexicon
/// [`LocalLexicon`]: crate::LocalLexicon
/// [`LocalLexicon::deserialize_in`]: crate::LocalLexicon::deserialize_in
/// [`ThreadedLexicon`]: crate::ThreadedLexicon
/// [`ThreadedLexicon::deserialize_in`]: crate::ThreadedLexicon::deserialize_in
/// [`deserialize_in`]: DeserializeIn::deserialize_in
pub trait DeserializeIn<'de, I: Lexicon + ?Sized>: Sized {
    /// Deserialize `Self`, interning string handles into `interner`.
    ///
    /// # Errors
    ///
    /// Returns an error from the deserializer when the input is invalid.
    fn deserialize_in<D>(interner: &mut I, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>;
}
