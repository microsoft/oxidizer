// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use serde::Serializer;

use crate::Reader;

/// Reader-aware serialization, the counterpart to [`DeserializeIn`](crate::de::DeserializeIn).
///
/// Serde's [`serde::Serialize`] carries no context, so a [`Sym`](crate::Sym) can
/// only be serialized as its raw integer handle — meaningless without the
/// matching interner. `SerializeIn` instead receives a [`Reader`] and resolves
/// every [`Sym`](crate::Sym) to the string it stands for, producing a
/// self-describing encoding: serialize with `SerializeIn` threading a
/// [`Reader`], then deserialize with
/// [`DeserializeIn`](crate::de::DeserializeIn) threading a fresh interner to
/// recover a value whose resolved strings compare identically to the original.
///
/// The round trip preserves *strings*, not numeric handles. A fresh interner
/// assigns handles in value-traversal order — which need not match the source
/// insertion order, and is nondeterministic for the `HashMap`/`HashSet` impls —
/// so a reconstructed [`Sym`](crate::Sym) generally has a different numeric
/// value. To rebuild a corpus with *identical* handles, serialize the whole
/// [`Reader`] with [`SerializeReader`](crate::se::SerializeReader), which emits
/// every string in handle order.
///
/// Most users derive [`SerializeIn`](derive@crate::se::SerializeIn) on their structs. To
/// serialize a value through an ordinary Serde entry point, wrap it in
/// [`SerializeInWith`](crate::se::SerializeInWith).
///
/// ```
/// use internity::se::{SerializeIn, SerializeInWith};
/// use internity::{LocalLexicon, Reader};
///
/// #[derive(SerializeIn)]
/// struct Record {
///     name: internity::Sym,
///     count: u64,
/// }
///
/// let mut lexicon = LocalLexicon::new();
/// let record = Record {
///     name: lexicon.intern("widget"),
///     count: 3,
/// };
/// let reader = lexicon.freeze();
/// let json = serde_json::to_string(&SerializeInWith::new(&record, &reader)).unwrap();
/// assert_eq!(json, r#"{"name":"widget","count":3}"#);
/// ```
pub trait SerializeIn<R: Reader + ?Sized> {
    /// Serialize `self`, resolving every [`Sym`](crate::Sym) against `reader`.
    ///
    /// Every [`Sym`](crate::Sym) must have been produced by the same interner that
    /// `reader` was frozen from. Passing a `Sym` from a different interner is a
    /// logic error: if its handle happens to be in range for `reader` it resolves
    /// to an unrelated string and serializes without error (see the crate-level
    /// note on cross-interner handles).
    ///
    /// # Errors
    ///
    /// Returns an error from the serializer, or a custom error if a
    /// [`Sym`](crate::Sym) holds an out-of-range or otherwise unresolvable handle
    /// for `reader`.
    fn serialize_in<S>(&self, reader: &R, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer;
}
